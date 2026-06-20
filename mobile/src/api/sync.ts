// Mobile sync api-client — the on-device cross-device sync surface (a full
// desktop peer: the same sync-engine, over the statically-embedded sync
// adapters, credentials via the keychain bridge). JSON passthrough over the
// Host's sync methods, wire shapes identical to the desktop.

import CalFfi from '../../modules/cal-ffi';

/** The active sync target. Mirrors the desktop `SyncAdapterConfig` enum; the
 *  password-only network kinds (webdav, ftp) + the OAuth kinds (dropbox,
 *  googledrive) join the local-filesystem kind. SFTP (host-key trust flow)
 *  follows. `password` is optional on re-edit: omit/empty to reuse the stored
 *  secret. The OAuth kinds require a prior {@link completeSyncOauth} (the refresh
 *  token is read from the keychain at configure time). */
export type SyncAdapterConfig =
  | { kind: 'local'; path: string }
  | { kind: 'webdav'; url: string; user: string; password?: string }
  | {
      kind: 'ftp';
      host: string;
      port?: number;
      user: string;
      path?: string;
      /** TLS handshake timing: `'explicit'` (default), `'implicit'`, `'plain'`. */
      mode?: 'explicit' | 'implicit' | 'plain';
      password?: string;
    }
  | { kind: 'dropbox'; client_id: string; client_secret?: string; path?: string }
  | {
      kind: 'googledrive';
      client_id: string;
      client_secret: string;
      folder_name?: string;
    }
  | {
      kind: 'sftp';
      host: string;
      port?: number;
      user: string;
      path: string;
      /** `'password'` (default) or `'key'` (SSH private key). */
      auth_method?: 'password' | 'key';
      /** password-auth: omit/empty to reuse the stored secret. */
      password?: string;
      /** key-auth: path to the private key file on the device. */
      key_path?: string;
      /** key-auth: the key's passphrase; omit/empty to reuse the stored one. */
      key_passphrase?: string;
    };

/** §19.5 host-key preview: the freshly-probed SFTP fingerprint + how it relates
 *  to the device's pin store. The UI shows the trust dialog per `status`. */
export interface HostKeyPreview {
  host_port: string;
  fingerprint: string;
  status:
    | { kind: 'new' }
    | { kind: 'unchanged' }
    | { kind: 'changed'; stored: string };
}

/** Probe an SFTP server's SHA256 host-key fingerprint and classify it against
 *  the device's pin store (network). On `new`/`changed` show the §19.5 trust
 *  dialog, then {@link trustSftpHostKey}; on `unchanged` configure directly. */
export const previewSftpHostKey = async (
  host: string,
  port: number,
): Promise<HostKeyPreview> =>
  JSON.parse(
    await CalFfi.previewSftpHostKeyJson(JSON.stringify({ host, port })),
  ) as HostKeyPreview;

/** Pin a user-confirmed fingerprint for `hostPort` (first-use or key-change). */
export const trustSftpHostKey = (
  hostPort: string,
  fingerprint: string,
): Promise<void> => CalFfi.trustSftpHostKey(hostPort, fingerprint);

/** Drop the pinned fingerprint for `hostPort` (the next connect re-prompts). */
export const forgetSftpHostKey = (hostPort: string): Promise<void> =>
  CalFfi.forgetSftpHostKey(hostPort);

/** The pinned fingerprint for `hostPort`, or null. No network. */
export const pinnedSftpHostKey = (hostPort: string): Promise<string | null> =>
  CalFfi.pinnedSftpHostKey(hostPort);

/** The statically-embedded sync-adapter plugin ids the Host drives OAuth for. */
export const SYNC_OAUTH_PLUGIN_IDS: Record<'dropbox' | 'googledrive', string> = {
  dropbox: 'com.aperio.sync-adapter-dropbox',
  googledrive: 'com.aperio.sync-adapter-googledrive',
};

/** Token-exchange inputs for {@link completeSyncOauth} (the redirect's code +
 *  the PKCE verifier/state from `beginOauth`). */
export interface CompleteSyncOauthRequest {
  client_id: string;
  /** Dropbox: optional (PKCE public app). Google Drive: required. */
  client_secret?: string | null;
  code: string;
  pkce_verifier: string;
  state: string;
  returned_state: string;
  redirect_uri: string;
}

/** Complete a sync-target OAuth: exchange the redirect's code for tokens, then
 *  store the refresh token in the adapter's keychain slot (no account is
 *  created). Follow with {@link configureSyncAdapter} to activate the target. */
export const completeSyncOauth = (
  pluginId: string,
  request: CompleteSyncOauthRequest,
): Promise<void> =>
  CalFfi.completeSyncOauthJson(pluginId, JSON.stringify(request));

/** Read-only engine state (the desktop `SyncStatus` shape). */
export interface SyncStatus {
  configured: boolean;
  in_flight: boolean;
  last_synced_at: string | null;
  interval_minutes: number;
  e2e_enabled: boolean;
  schema_too_old: boolean;
  min_app_version_required: string | null;
  sustained_failure: boolean;
  stale_device_since: string | null;
  /** The latched error code from the last failed round (the desktop
   *  `last_error_code`). `'encryption_required'` means a peer turned on E2E and
   *  this device must {@link adoptRemoteEncryption} with the passphrase. */
  last_error_code: string | null;
}

/** Outcome of one sync round (the desktop `SyncRoundReport` shape). */
export interface SyncRoundReport {
  pushed_logs: number;
  fetched_logs: number;
  applied: number;
  skipped_own: number;
  skipped_already_applied: number;
  skipped_unsupported: number;
  apply_failures: number;
  push_failures: number;
  conflicts: number;
}

/** Set the active sync target (persists + probes the connection). */
export const configureSyncAdapter = (config: SyncAdapterConfig): Promise<void> =>
  CalFfi.configureSyncAdapterJson(JSON.stringify(config));

/** A non-secret summary of the configured sync target (kind + a human detail
 *  string), for the "connected" card. `null` when nothing is configured. */
export interface SyncAdapterSummary {
  kind: string;
  detail: string;
}

/** Read the configured target's non-secret summary (kind + detail), or `null`. */
export const getSyncAdapterSummary = async (): Promise<SyncAdapterSummary | null> =>
  JSON.parse(await CalFfi.getSyncAdapterSummaryJson()) as SyncAdapterSummary | null;

/** Disconnect the configured sync target. Keeps the entered fields + secrets so
 *  reconnecting is one tap; the form re-shows once syncStatus reads back
 *  `configured: false`. */
export const disconnectSync = (): Promise<void> => CalFfi.disconnectSync();

/** The current engine status (no round). */
export const syncStatus = async (): Promise<SyncStatus> =>
  JSON.parse(await CalFfi.syncStatusJson()) as SyncStatus;

/** What kicked off a sync round — recorded in the sync log (the desktop
 *  `SyncTrigger`). Mobile has no scheduler, so the caller tags the context:
 *  `'manual'` (the Settings button), `'app_start'`/`'periodic'` (launch /
 *  foreground full rounds), `'kick'` (debounced push after a mutation),
 *  `'app_exit'` (the background flush). */
export type SyncTrigger =
  | 'manual'
  | 'app_start'
  | 'periodic'
  | 'kick'
  | 'app_exit';

/** Run one sync round (push + fetch + apply); rejects "not configured" until a
 *  target is set. `trigger` (default `'manual'`) tags the round in the log. */
export const syncNow = async (
  trigger: SyncTrigger = 'manual',
): Promise<SyncRoundReport> =>
  JSON.parse(await CalFfi.syncNowJson(trigger)) as SyncRoundReport;

/** Push local pending logs without fetching (call on app background). `trigger`
 *  (default `'kick'`) tags the round in the log. */
export const pushNow = (trigger: SyncTrigger = 'kick'): Promise<number> =>
  CalFfi.pushNow(trigger);

// ── Sync log (Protokoll) ──
// Every sync round records a row (mobile has no scheduler, so sync_now/push_now
// self-record). The viewer lists recent rounds; clearing scrubs the history.

/** One recorded sync round (the desktop `SyncLogEntry` shape). Success rows
 *  carry the counters; a failed row carries `error` instead. */
export interface SyncLogEntry {
  id: number;
  recorded_at: string;
  trigger: string;
  success: boolean;
  pushed_logs: number | null;
  fetched_logs: number | null;
  applied: number | null;
  conflicts: number | null;
  duration_ms: number | null;
  error: string | null;
}

/** Recent sync-log rows (newest first), capped at `limit` (default 100). */
export const listSyncLog = async (limit = 100): Promise<SyncLogEntry[]> =>
  JSON.parse(await CalFfi.listSyncLogJson(limit)) as SyncLogEntry[];

/** Clear the sync-log history. */
export const clearSyncLog = (): Promise<void> => CalFfi.clearSyncLog();

/** Outcome of a compaction round (§19.10) — the counters the UI renders. */
export interface CompactionReport {
  snapshot_timestamp: string | null;
  deleted_logs: number;
  failed_deletes: number;
  stale_devices: number;
  snapshot_rows: number;
  snapshot_settings: number;
}

/** Run a compaction round now: snapshot the local state + GC log files older
 *  than the new horizon. Rejects when no sync target is configured. Compaction
 *  also runs automatically at the §19.10 thresholds; this is the manual
 *  override. The outcome is also recorded in the sync log. */
export const compactNow = async (): Promise<CompactionReport> =>
  JSON.parse(await CalFfi.compactNowJson()) as CompactionReport;

// ── External cache (SWR) controls ──
// External reads already serve stale-while-revalidate + self-warm on a
// cold/stale read; these are the explicit controls (the desktop's cache surface):
// a manual "refresh now", a "last updated" status, and an on-foreground warm.

/** The external-cache warm-pass status (the desktop `CacheRefreshStatus`). */
export interface CacheRefreshStatus {
  /** True while a warm pass is running. */
  refreshing: boolean;
  /** RFC3339 of the last completed pass, or null. */
  last_refreshed_at: string | null;
}

/** Kick an immediate warm pass over every external account's containers (the
 *  manual "refresh now"). Fire-and-forget — poll {@link cacheRefreshStatus} for
 *  completion, or refocus the view to pick up the warmed data. */
export const refreshExternalCache = (): Promise<void> =>
  CalFfi.refreshExternalCache();

/** The external-cache warm status — drives a "refreshing…" spinner + a "last
 *  updated" line. */
export const cacheRefreshStatus = async (): Promise<CacheRefreshStatus> =>
  JSON.parse(await CalFfi.getCacheRefreshStatusJson()) as CacheRefreshStatus;

/** Warm the external cache on app-foreground (the mobile stand-in for the
 *  desktop periodic warm loop). Fire-and-forget. */
export const warmCacheOnForeground = (): Promise<void> =>
  CalFfi.warmCacheOnForeground();

// ── Sync conflicts ──
// Field-level conflicts the applier recorded (a field edited differently on two
// devices). The values are JSON-encoded scalars — decode with JSON.parse for
// display. Resolution is per-device bookkeeping + (take_remote) a local write.

export type SyncResolutionChoice = 'keep_local' | 'take_remote' | 'save_both';

export interface SyncConflict {
  id: number;
  detected_at: string;
  row_kind: 'event' | 'task' | 'task_list' | 'calendar' | 'color_label';
  row_id: string;
  field: string;
  local_value: string | null;
  remote_value: string | null;
  remote_device_id: string;
  remote_timestamp: string;
  resolved: boolean;
  resolution: SyncResolutionChoice | null;
  resolved_at: string | null;
}

/** Count of unresolved conflicts (the entry-point badge). */
export const syncConflictCount = (): Promise<number> => CalFfi.syncConflictCount();

/** Resume a device flagged stale (`status.stale_device_since` set): re-onboard
 *  from the configured target + clear the stale flag. Returns the
 *  OnboardingReport; rejects when no target is configured. */
export const resumeStaleDevice = async (): Promise<OnboardingReport> =>
  JSON.parse(await CalFfi.resumeStaleDeviceJson()) as OnboardingReport;

/** Every unresolved conflict. */
export const listSyncConflicts = async (): Promise<SyncConflict[]> =>
  JSON.parse(await CalFfi.listSyncConflictsJson()) as SyncConflict[];

/** Apply a resolution. `take_remote` writes the remote value + emits an update
 *  SyncEvent (carried out on the next push/round); `save_both` rejects. */
export const resolveSyncConflict = (
  id: number,
  choice: SyncResolutionChoice,
): Promise<void> => CalFfi.resolveSyncConflict(id, choice);

/** Enable end-to-end encryption (§19.7) on the configured sync target: mint a
 *  fresh passphrase-protected key, write the encrypted dataset, and encrypt
 *  every subsequent round. The key is device-local (never synced). Irreversible
 *  without the passphrase — losing it means losing the data. */
export const enableSyncEncryption = async (passphrase: string): Promise<void> => {
  await CalFfi.enableSyncEncryptionJson(passphrase);
};

/** Adopt encryption a PEER turned on (§19.7): when a sync round fails because
 *  another device enabled E2E (status `last_error_code === 'encryption_required'`),
 *  call this with the dataset passphrase to derive the key, switch this device to
 *  encrypted mode, and unblock syncing. Mirrors the desktop `adopt_remote_encryption`. */
export const adoptRemoteEncryption = async (passphrase: string): Promise<void> => {
  await CalFfi.adoptRemoteEncryptionJson(passphrase);
};

/** Counters from {@link disableSyncEncryption} (the desktop `DisableE2eReport`):
 *  how much of the dataset was rewritten as plaintext. */
export interface DisableE2eReport {
  logs_rewritten: number;
  snapshot_rewritten: boolean;
}

/** Disable E2E on the configured (encrypted) target: verify the passphrase, then
 *  rewrite every log + snapshot as plaintext (stripping account secrets), flip
 *  the dataset metadata to plaintext, and drop this device's key. Irreversible
 *  for the cluster — every OTHER device must re-onboard afterwards. */
export const disableSyncEncryption = async (
  passphrase: string,
): Promise<DisableE2eReport> =>
  JSON.parse(
    await CalFfi.disableSyncEncryptionJson(passphrase),
  ) as DisableE2eReport;

/** Rotate the E2E passphrase on the configured (encrypted) target: verify the
 *  current passphrase, re-wrap the SAME data key under a fresh new-passphrase
 *  key, and push the updated dataset metadata. The data itself is untouched —
 *  devices already syncing keep working; only devices that JOIN from here on
 *  need the new passphrase. Rejects when the target isn't encrypted. */
export const changeSyncPassphrase = async (
  oldPassphrase: string,
  newPassphrase: string,
): Promise<void> => {
  await CalFfi.changeSyncPassphraseJson(oldPassphrase, newPassphrase);
};

/** One device row in an Existing {@link SyncPreview} (the desktop
 *  `DeviceSummary`). */
export interface SyncDeviceSummary {
  id: string;
  name: string | null;
  last_seen_log: string;
  app_version: string;
  stale: boolean;
  is_this_device: boolean;
}

/** How the running build relates to a dataset's version requirements (the
 *  desktop `Compatibility`). Anything but `ok` gates the join. */
export type SyncCompatibility =
  | { kind: 'ok' }
  | { kind: 'app_too_old'; required: string; running: string }
  | { kind: 'schema_ahead'; remote: number; local: number };

/** Outcome of probing a target ({@link previewSyncTarget}) — the desktop
 *  `SyncPreview`. `empty` = you're the first device (offer "start fresh");
 *  `existing` = a dataset is already there (offer "join" — pass the passphrase
 *  when `e2e_enabled`). */
export type SyncPreview =
  | { kind: 'empty' }
  | {
      kind: 'existing';
      schema_version: number;
      min_app_version: string;
      snapshot_timestamp: string | null;
      e2e_enabled: boolean;
      devices: SyncDeviceSummary[];
      compatibility: SyncCompatibility;
    };

/** Outcome of joining a dataset ({@link acceptRemoteDataset}) — the desktop
 *  `OnboardingReport` (no push counts; onboarding adopts, never pushes). */
export interface OnboardingReport {
  fetched_logs: number;
  applied: number;
  skipped_own: number;
  skipped_already_applied: number;
  skipped_unsupported: number;
  apply_failures: number;
  remote_was_empty: boolean;
  device_count: number;
}

/** Probe a target WITHOUT committing: returns Empty (fresh) or Existing (a
 *  dataset is already present). Side-effect-free — render the join-vs-overwrite
 *  choice from `kind`, and require a passphrase before joining when
 *  `e2e_enabled`. */
export const previewSyncTarget = async (
  config: SyncAdapterConfig,
): Promise<SyncPreview> =>
  JSON.parse(
    await CalFfi.previewSyncTargetJson(JSON.stringify(config)),
  ) as SyncPreview;

/** Join an EXISTING dataset: pull + apply its snapshot + logs, register this
 *  device, then activate + persist the target. Pass `passphrase` when the
 *  dataset is end-to-end encrypted — it's how this device derives the key (a
 *  fresh device can't read an encrypted target any other way). */
export const acceptRemoteDataset = async (
  config: SyncAdapterConfig,
  deviceName: string | null,
  passphrase: string | null,
): Promise<OnboardingReport> =>
  JSON.parse(
    await CalFfi.acceptRemoteDatasetJson(
      JSON.stringify(config),
      deviceName,
      passphrase,
    ),
  ) as OnboardingReport;

/** "Start fresh" (§19.11): overwrite the target's meta.json so it names ONLY
 *  this device, optionally enabling end-to-end encryption from `passphrase`
 *  (null/blank = a plaintext fresh dataset — the key is minted at creation when
 *  a passphrase is given), then activate + persist. The unified Connect button
 *  uses it to INITIALISE an empty target; the destructive "overwrite" action
 *  (behind a confirm) uses it on an existing target with `passphrase = null`. */
export const adoptLocalDataset = async (
  config: SyncAdapterConfig,
  deviceName: string | null,
  passphrase: string | null = null,
): Promise<OnboardingReport> =>
  JSON.parse(
    await CalFfi.adoptLocalDatasetJson(
      JSON.stringify(config),
      deviceName,
      passphrase,
    ),
  ) as OnboardingReport;
