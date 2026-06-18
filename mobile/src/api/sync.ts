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

/** The current engine status (no round). */
export const syncStatus = async (): Promise<SyncStatus> =>
  JSON.parse(await CalFfi.syncStatusJson()) as SyncStatus;

/** Run one sync round (push + fetch + apply); rejects "not configured" until a
 *  target is set. */
export const syncNow = async (): Promise<SyncRoundReport> =>
  JSON.parse(await CalFfi.syncNowJson()) as SyncRoundReport;

/** Push local pending logs without fetching (call on app background). */
export const pushNow = (): Promise<number> => CalFfi.pushNow();

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
