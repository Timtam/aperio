// Mobile sync api-client — the on-device cross-device sync surface (a full
// desktop peer: the same sync-engine, over the statically-embedded sync
// adapters, credentials via the keychain bridge). JSON passthrough over the
// Host's sync methods, wire shapes identical to the desktop.

import CalFfi from '../../modules/cal-ffi';

/** The active sync target. Mirrors the desktop `SyncAdapterConfig` enum; the
 *  password-only network kinds (webdav, ftp) join the local-filesystem kind.
 *  SFTP (host-key trust flow) + the OAuth kinds (Dropbox / Google Drive) follow.
 *  `password` is optional on re-edit: omit/empty to reuse the stored secret. */
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
    };

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
