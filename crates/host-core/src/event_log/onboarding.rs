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

use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::Serialize;
use sync_core::{
    DeviceCursor, DeviceId, DeviceRecord, LogFile, LogFileName, MetaJson, SyncAdapter, SyncError,
    SyncResult,
};
use tracing::{debug, info, warn};

use crate::db::SharedConn;
use crate::event_log::{ApplyReport, EventLogApplier, SnapshotBuilder, SYNC_CURSOR_PREF_KEY};
use crate::user_prefs::UserPrefsRepo;

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
    pub last_seen_log: String,
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
    pub skipped_own: usize,
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
}

impl OnboardingReport {
    fn merge_apply(&mut self, report: ApplyReport) {
        self.applied += report.applied;
        self.skipped_own += report.skipped_own;
        self.skipped_already_applied += report.skipped_already_applied;
        self.skipped_unsupported += report.skipped_unsupported;
        self.apply_failures += report.failed;
    }
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
                    devices: meta
                        .devices
                        .iter()
                        .map(|(id, rec)| DeviceSummary {
                            id: id.clone(),
                            name: rec.name.clone(),
                            last_seen_log: rec.last_seen_log.to_rfc3339(),
                            app_version: rec.app_version.clone(),
                            stale: rec.stale,
                            is_this_device: id == self.local_device_id.as_str(),
                        })
                        .collect(),
                    compatibility,
                }
            }
        })
    }

    /// "Datensatz übernehmen" path. Pulls every log from the remote,
    /// hands them to the applier, registers this device in meta.json,
    /// advances the local sync cursor.
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
        let mut meta = adapter.fetch_meta().await?.ok_or_else(|| {
            SyncError::not_found("remote has no meta.json — use adopt_local for a fresh dataset")
        })?;

        // Phase Sl: refuse to pull from a dataset our build can't
        // safely read. Surfaces as `SchemaTooOld` so the frontend
        // can render the §19.13 "update required" modal instead of
        // a generic error.
        sync_core::ensure_compatible(&meta, &self.app_version)?;

        // Phase Sg: consume snapshot first if one exists. Apply
        // its body to local SQLite + advance the cursor to the
        // snapshot timestamp; the log pull below then only fetches
        // the small backlog that came after compaction.
        //
        // We start the cursor at MIN_UTC (so a missing snapshot
        // means "fetch everything"). A successful snapshot apply
        // bumps it to the snapshot's own timestamp.
        let mut starting_cursor: DateTime<Utc> = DateTime::<Utc>::MIN_UTC;
        let mut snapshot_applied = false;
        match adapter.fetch_snapshot().await? {
            Some(snapshot) => {
                info!(
                    snapshot_ts = %snapshot.metadata.snapshot_timestamp,
                    "consuming remote snapshot during onboarding",
                );
                match self.snapshot_builder.apply(&snapshot) {
                    Ok(_outcome) => {
                        starting_cursor = snapshot.metadata.snapshot_timestamp;
                        snapshot_applied = true;
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
                debug!("no remote snapshot present; falling back to full log replay",);
            }
        }

        // Pull logs newer than the snapshot timestamp (or epoch if
        // there was no snapshot). The applier's idempotency table
        // guarantees re-applying old logs is a no-op anyway, but
        // skipping them saves the disk + serde cost.
        let logs = adapter
            .fetch_new_logs(&DeviceCursor {
                last_seen_log: starting_cursor,
                // Own files are discarded by the filter below anyway —
                // let the adapter skip fetching their bytes.
                exclude_device: Some(self.local_device_id.clone()),
                // Onboarding/resume fetch from scratch — no applied
                // lengths recorded yet, growth-refetch not relevant.
                known_lengths: Vec::new(),
            })
            .await?;
        let foreign: Vec<_> = logs
            .into_iter()
            .filter(|log| log.name.device_id != self.local_device_id)
            .collect();
        let fetched_logs = foreign.len();
        info!(
            count = fetched_logs,
            snapshot_applied = snapshot_applied,
            "accept_remote pulled remote logs",
        );

        // Track the newest timestamp so we can set the cursor
        // correctly afterwards. Default to whatever the snapshot
        // gave us (epoch when there was none) so an empty remote
        // still lands a sensible cursor.
        let mut newest: DateTime<Utc> = starting_cursor;

        let mut report = OnboardingReport {
            remote_was_empty: false,
            ..Default::default()
        };
        report.fetched_logs = fetched_logs;

        let mut applied_lengths: Vec<(String, u64)> = Vec::new();
        for log in &foreign {
            if log.name.timestamp > newest {
                newest = log.name.timestamp;
            }
            match self.applier.apply_log_file(log) {
                Ok(apply_report) => {
                    report.merge_apply(apply_report);
                    applied_lengths.push((log.name.to_filename(), log.bytes.len() as u64));
                }
                Err(err) => {
                    warn!(
                        log = %log.name.to_filename(),
                        ?err,
                        "applier failed during onboarding",
                    );
                    report.apply_failures += 1;
                    // Length 0 → any listed size counts as grown, so the
                    // next round retries the file (matches the round's
                    // failed-apply policy).
                    applied_lengths.push((log.name.to_filename(), 0));
                }
            }
        }
        // Seed the growth-refetch records for what we just applied —
        // typically including the peers' LIVE session files, whose later
        // appends would otherwise stay invisible until those peers rotate
        // (the cursor lands exactly on their timestamps).
        self.record_applied_lengths(&applied_lengths);

        // Persist the cursor so the next scheduler round doesn't
        // re-pull the same backlog. The snapshot path bumps
        // `newest` even when no logs followed it; if we never
        // bumped `newest` past MIN_UTC (no snapshot AND no logs),
        // don't touch the existing cursor.
        if newest > DateTime::<Utc>::MIN_UTC {
            self.save_cursor(newest)?;
        }

        // Persist + push the device registration.
        self.register_self_in_meta(&mut meta, device_name);
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
    /// device was offline for a while" dialog. Sequence:
    ///
    /// 1. Re-pull the current `snapshot.json` + apply via
    ///    `SnapshotBuilder` (upserts rows from the snapshot
    ///    body; local-only rows untouched).
    /// 2. Pull + apply any foreign logs newer than the snapshot.
    /// 3. Replay our own pending logs through the applier's
    ///    force-own path. The applier's field-level merge
    ///    handles the snapshot-vs-offline-edit ordering: if our
    ///    pending event's timestamp is newer than the snapshot's
    ///    `updated_at` for that row, our value wins; otherwise
    ///    the snapshot value stays. This step is what brings
    ///    back any local edits the user made while offline that
    ///    the snapshot apply would otherwise have clobbered.
    /// 4. Save the cursor + clear the device's `stale` flag in
    ///    meta + push the updated meta.
    ///
    /// Pending logs stay on disk after the replay; the next
    /// scheduled sync round picks them up and pushes to the
    /// remote so other devices receive them too.
    pub async fn resume_from_stale(
        &self,
        adapter: &dyn SyncAdapter,
    ) -> SyncResult<OnboardingReport> {
        adapter.test_connection().await?;
        let mut meta = adapter.fetch_meta().await?.ok_or_else(|| {
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
        let mut starting_cursor = match adapter.fetch_snapshot().await? {
            Some(snapshot) => {
                info!(
                    snapshot_ts = %snapshot.metadata.snapshot_timestamp,
                    "applying snapshot during stale-device resume",
                );
                self.snapshot_builder.apply(&snapshot).map_err(|err| {
                    SyncError::internal(format!("stale resume snapshot apply: {err}"))
                })?;
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

        // Pull + apply any logs that landed after the snapshot.
        let logs = adapter
            .fetch_new_logs(&DeviceCursor {
                last_seen_log: starting_cursor,
                // Own files are discarded by the filter below anyway —
                // let the adapter skip fetching their bytes.
                exclude_device: Some(self.local_device_id.clone()),
                // Onboarding/resume fetch from scratch — no applied
                // lengths recorded yet, growth-refetch not relevant.
                known_lengths: Vec::new(),
            })
            .await?;
        let foreign: Vec<_> = logs
            .into_iter()
            .filter(|log| log.name.device_id != self.local_device_id)
            .collect();
        let fetched_logs = foreign.len();
        info!(
            count = fetched_logs,
            "stale resume pulled post-snapshot logs",
        );

        let mut report = OnboardingReport {
            remote_was_empty: false,
            ..Default::default()
        };
        report.fetched_logs = fetched_logs;

        let mut applied_lengths: Vec<(String, u64)> = Vec::new();
        for log in &foreign {
            if log.name.timestamp > starting_cursor {
                starting_cursor = log.name.timestamp;
            }
            match self.applier.apply_log_file(log) {
                Ok(apply_report) => {
                    report.merge_apply(apply_report);
                    applied_lengths.push((log.name.to_filename(), log.bytes.len() as u64));
                }
                Err(err) => {
                    warn!(
                        log = %log.name.to_filename(),
                        ?err,
                        "applier failed during stale resume",
                    );
                    report.apply_failures += 1;
                    applied_lengths.push((log.name.to_filename(), 0));
                }
            }
        }
        // Seed growth-refetch records (see accept_remote): the peers'
        // live session files just applied would otherwise never be
        // re-fetched when they grow.
        self.record_applied_lengths(&applied_lengths);

        // §19.10 v1.1: replay our own pending logs through the
        // applier's force-own path. The snapshot apply above
        // upserts every row from the snapshot dump — including
        // shared rows the user edited locally during the offline
        // window — so without this pass those local edits would
        // disappear locally until another device relayed them
        // back. The replay walks `pending/`, parses each .jsonl
        // session file, and feeds the envelopes through
        // `apply_envelopes_force_own`. The applier's field-level
        // merge handles the ordering: edits newer than the
        // snapshot row's `updated_at` win, edits older than the
        // snapshot stay overwritten (matches the convergent
        // semantics other devices would compute against the
        // remote logs).
        //
        // Failures are non-fatal — the file stays on disk and
        // the next sync round will push it; on the FOLLOWING
        // resume (unlikely but possible) we'd re-try the replay.
        self.replay_pending_logs(&mut report).await;

        // Persist the cursor before mutating meta. If we crash
        // between cursor + meta push the next sync round just
        // re-pulls the post-snapshot logs (idempotent applier).
        self.save_cursor(starting_cursor)?;

        // Clear our device's `stale` flag in meta.json + bump
        // `last_seen_log` so other devices see us as current.
        // This is the bit that lets a future compactor round
        // include our cursor in its retention math.
        if let Some(entry) = meta.devices.get_mut(self.local_device_id.as_str()) {
            entry.stale = false;
            entry.last_seen_log = starting_cursor;
        }
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

    /// Walk `pending_dir` and re-apply every JSONL session file
    /// through `apply_envelopes_force_own`. Used by stale resume
    /// to restore offline edits over a freshly-applied snapshot.
    ///
    /// Per-file failures degrade gracefully: a parse error or
    /// read error gets logged + skipped, the rest of the
    /// directory is still processed, and the apply counts go
    /// into the report. The files themselves stay on disk so
    /// the next sync round can still push them to the remote.
    async fn replay_pending_logs(&self, report: &mut OnboardingReport) {
        let mut entries = match tokio::fs::read_dir(&self.pending_dir).await {
            Ok(rd) => rd,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                // No writer has flushed anything this install —
                // nothing to replay.
                debug!("pending dir empty during stale resume; skipping replay");
                return;
            }
            Err(err) => {
                warn!(?err, "couldn't open pending dir during stale resume");
                return;
            }
        };
        let mut replayed_files = 0usize;
        loop {
            let entry = match entries.next_entry().await {
                Ok(Some(e)) => e,
                Ok(None) => break,
                Err(err) => {
                    warn!(?err, "read_dir entry error during replay");
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
                    debug!(
                        name = name,
                        "skipping pending entry during replay: not a log file"
                    );
                    continue;
                }
            };
            // Only OUR own-device logs make sense to replay —
            // anything else in the pending dir is suspicious
            // (the writer only emits our own session files) but
            // bypassing skip_own on a foreign log would
            // double-apply via the field-level merge. Guard
            // explicitly.
            if parsed.device_id != self.local_device_id {
                debug!(
                    name = name,
                    "skipping pending entry during replay: foreign device",
                );
                continue;
            }
            let bytes = match tokio::fs::read(&path).await {
                Ok(b) => b,
                Err(err) => {
                    warn!(name = name, ?err, "couldn't read pending log");
                    continue;
                }
            };
            let log = LogFile {
                name: parsed,
                bytes,
            };
            let envelopes = match log.into_envelopes() {
                Ok(e) => e,
                Err(err) => {
                    warn!(
                        name = name,
                        ?err,
                        "couldn't parse pending log during replay",
                    );
                    continue;
                }
            };
            match self.applier.apply_envelopes_force_own(envelopes) {
                Ok(apply_report) => {
                    replayed_files += 1;
                    report.merge_apply(apply_report);
                }
                Err(err) => {
                    warn!(
                        name = name,
                        ?err,
                        "force-own apply failed during stale resume",
                    );
                    report.apply_failures += 1;
                }
            }
        }
        info!(files = replayed_files, "stale resume replayed pending logs",);
    }

    /// "Neu beginnen" path. Builds a fresh meta.json with only this
    /// device, pushes it to the remote, then leaves the rest to the
    /// scheduler's next round (which will push whatever was already
    /// queued in `pending/`).
    ///
    /// Caller is responsible for warning the user that any existing
    /// remote data is now orphaned — the meta we write overwrites
    /// the prior file, and the prior logs become unreachable on
    /// next compaction.
    pub async fn adopt_local(
        &self,
        adapter: &dyn SyncAdapter,
        device_name: Option<&str>,
        e2e_params: Option<sync_core::EncryptionParams>,
    ) -> SyncResult<OnboardingReport> {
        adapter.test_connection().await?;

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

        let mut meta = MetaJson::fresh(&self.app_version);
        // Phase Sk: bake the E2E flag + params into the fresh
        // meta so devices joining later can derive the same key
        // from the user's passphrase. The key itself is NEVER
        // written here — it lives only in each device's keychain.
        if let Some(params) = e2e_params {
            meta.e2e_enabled = true;
            meta.e2e_params = Some(params);
        }
        self.register_self_in_meta(&mut meta, device_name);
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
        let is_current = |meta: &MetaJson| {
            meta.devices
                .get(self.local_device_id.as_str())
                .is_some_and(|d| {
                    d.last_seen_log == last_seen_log
                        && d.name == name
                        && d.app_version == self.app_version
                        && !d.stale
                })
        };
        if round_meta.is_some_and(&is_current) {
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
        if is_current(&meta) {
            return Ok(());
        }
        meta.upsert_device(
            &self.local_device_id,
            DeviceRecord {
                name,
                last_seen_log,
                app_version: self.app_version.clone(),
                stale: false,
            },
        );
        adapter.push_meta(&meta).await?;
        Ok(())
    }

    fn register_self_in_meta(&self, meta: &mut MetaJson, device_name: Option<&str>) {
        let name = device_name.map(str::to_string).or_else(|| {
            UserPrefsRepo::new(&self.db)
                .get(PREF_DEVICE_NAME)
                .ok()
                .flatten()
        });
        meta.upsert_device(
            &self.local_device_id,
            DeviceRecord {
                name,
                last_seen_log: Utc::now(),
                app_version: self.app_version.clone(),
                stale: false,
            },
        );
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

    /// Merge freshly applied log byte lengths into the shared
    /// growth-refetch map (same pref + semantics as the sync round —
    /// `sync_engine::merge_applied_log_lengths`). Best-effort: a failed
    /// write only costs a re-fetch.
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
    /// tolerant stale-resume path to catch up via logs when there's no
    /// snapshot to anchor on.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::DbHandle;
    use async_trait::async_trait;
    use cal_adapter_local::LocalAdapter;
    use std::sync::{Arc, Mutex};
    use sync_core::{LogFile, LogFileName, Snapshot};
    use tempfile::TempDir;

    /// Minimal in-memory `SyncAdapter` for unit testing the
    /// onboarding flow. Stores its bytes in `Mutex` so the test
    /// doesn't need a temp directory.
    struct FakeAdapter {
        meta: Mutex<Option<MetaJson>>,
        logs: Mutex<Vec<LogFile>>,
    }

    impl FakeAdapter {
        fn new() -> Self {
            Self {
                meta: Mutex::new(None),
                logs: Mutex::new(Vec::new()),
            }
        }

        fn with_logs(logs: Vec<LogFile>) -> Self {
            Self {
                meta: Mutex::new(None),
                logs: Mutex::new(logs),
            }
        }

        fn install_meta(&self, meta: MetaJson) {
            *self.meta.lock().unwrap() = Some(meta);
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
            Ok(self
                .logs
                .lock()
                .unwrap()
                .iter()
                .filter(|l| l.name.timestamp > since.last_seen_log)
                .cloned()
                .collect())
        }
        async fn push_log(&self, log: &LogFile) -> SyncResult<()> {
            self.logs.lock().unwrap().push(log.clone());
            Ok(())
        }
        async fn fetch_snapshot(&self) -> SyncResult<Option<Snapshot>> {
            Ok(None)
        }
        async fn push_snapshot(&self, _snapshot: &Snapshot) -> SyncResult<()> {
            Ok(())
        }
        async fn delete_log(&self, _name: &LogFileName) -> SyncResult<()> {
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
            Arc::new(sync_engine::test_support::FakeSecrets::default()),
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
                app_version: "1.0.0".into(),
                stale: false,
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
                app_version: "1.0.0".into(),
                stale: false,
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
                app_version: "1.0.0".into(),
                stale: false,
            },
        );
        meta.upsert_device(
            &DeviceId::from_string("dev-b".into()),
            DeviceRecord {
                name: Some("B".into()),
                last_seen_log: Utc::now(),
                app_version: "1.0.0".into(),
                stale: false,
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
        // we are about to stamp.
        let mut cached = MetaJson::fresh("1.0.0-test");
        cached.upsert_device(
            &DeviceId::from_string("dev-this".into()),
            DeviceRecord {
                name: None,
                last_seen_log: now,
                app_version: "1.0.0-test".into(),
                stale: false,
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
                app_version: "1.0.0-test".into(),
                stale: false,
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
                app_version: "1.0.0-test".into(),
                stale: false,
            },
        );
        // Remote meanwhile: the peer's write landed.
        let mut remote = cached.clone();
        remote.upsert_device(
            &DeviceId::from_string("dev-marker".into()),
            DeviceRecord {
                name: Some("Marker".into()),
                last_seen_log: now,
                app_version: "1.0.0-test".into(),
                stale: false,
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
                app_version: "1.0.0-test".into(),
                stale: false,
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
                app_version: "1.0.0".into(),
                stale: true,
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
    async fn accept_remote_advances_local_cursor_past_newest_log() {
        let (_tmp, db) = fresh_db();
        let svc = build_service(db.shared());

        // Pre-seed: one log from another device with a known
        // timestamp. The applier will reject any unsupported event
        // types as "skipped", which is fine — the test asserts on
        // cursor advance, not on row counts.
        let ts: DateTime<Utc> = "2026-05-01T12:34:56Z".parse().unwrap();
        // Hard-code an envelope id rather than minting one — the
        // applier doesn't validate id shape, and `mint_event_id` is
        // crate-private.
        let envelope_json = format!(
            r#"{{"id":"ev-test-001","device_id":"dev-other","timestamp":"{}","type":"event.deleted","id_payload":{{"id":"nonexistent"}}}}"#,
            ts.to_rfc3339(),
        );
        let log = LogFile {
            name: LogFileName::new(ts, DeviceId::from_string("dev-other".into())),
            bytes: envelope_json.into_bytes(),
        };
        let adapter = FakeAdapter::with_logs(vec![log]);
        adapter.install_meta(MetaJson::fresh("1.0.0"));

        svc.accept_remote(&adapter, None).await.unwrap();

        let cursor = UserPrefsRepo::new(&db.shared())
            .get(SYNC_CURSOR_PREF_KEY)
            .unwrap()
            .expect("cursor should be persisted");
        let parsed: DateTime<Utc> = cursor.parse().unwrap();
        assert_eq!(parsed, ts);
    }
}
