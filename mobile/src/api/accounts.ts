// Mobile accounts api-client — the engine-reuse boundary for the account
// surface (the Host: statically-embedded adapter plugins + the keychain-bridged
// SecretStore). Mirrors the desktop's account command shapes so ported logic
// reads the same; each body is JSON passthrough over a `CalFfi.*` Host call.
//
// The JSON wire is the `cal_core`/desktop serde shape (snake_case), so the
// payloads match the desktop's Tauri commands exactly. `Account` is defined
// here (not yet in @aperio/shared) to keep this increment off the desktop;
// it hoists to the shared package when the desktop account UI is ported.

import CalFfi from '../../modules/cal-ffi';

import { scheduleBackgroundPush } from './syncTriggers';

/** Adapter kinds the engine knows. Snake_case to match the Rust serde form. */
export type AdapterKind =
  | 'local'
  | 'caldav'
  | 'ical'
  | 'google'
  | 'microsoft_graph'
  | 'ews'
  | 'vikunja'
  | 'todoist'
  | 'zoom'
  | 'teams'
  | 'meet'
  | 'webex';

/** A persisted account row (the desktop `Account` wire shape). */
export interface Account {
  id: string;
  adapter_kind: AdapterKind;
  display_name: string;
  /** Adapter-specific non-secret config, as a JSON string. */
  config_json: string;
  created_at: string;
  updated_at: string;
}

/** Create-account request — the desktop `CreateAccountRequest` wire shape. */
export interface CreateAccountRequest {
  adapter_kind: AdapterKind;
  display_name: string;
  /** Adapter-specific non-secret config as a JSON string (default `{}`). */
  config_json?: string;
  /** The secret half (CalDAV password, API token, …); stored only in the
   *  platform keychain, never in SQLite. Omit for the local account. */
  secret?: string | null;
}

/** All persisted accounts, in creation order. */
export const listAccounts = async (): Promise<Account[]> =>
  JSON.parse(await CalFfi.accountsJson()) as Account[];

/** Create an account: persists the row, stores the secret via the keychain
 *  bridge, and registers the adapter. Rejects (typed store error) for OAuth
 *  kinds (a later phase) and on bad config / registration failure. */
export const createAccount = async (
  request: CreateAccountRequest,
): Promise<Account> => {
  const created = JSON.parse(
    await CalFfi.createAccountJson(JSON.stringify(request)),
  ) as Account;
  scheduleBackgroundPush();
  return created;
};

/** Delete an account (unregister adapter + clear secrets + drop row). Rejects
 *  when deleting the implicit local account. */
export const deleteAccount = async (id: string): Promise<void> => {
  await CalFfi.deleteAccount(id);
  scheduleBackgroundPush();
};
