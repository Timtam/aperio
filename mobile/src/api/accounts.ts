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
  | 'device_calendar'
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

/** Probe entered credentials WITHOUT persisting anything — opens an ephemeral
 *  adapter and runs the kind's read probe. Resolves if the credentials work,
 *  rejects with the typed store error otherwise. Reuses the create-account
 *  request shape (display_name is ignored). */
export const testAccount = async (request: CreateAccountRequest): Promise<void> => {
  await CalFfi.testAccountJson(JSON.stringify(request));
};

/** Run the OS calendar/reminders permission prompt for the device-calendar
 *  adapter — the add-account "grant access" step. Resolves `true` iff access was
 *  granted (then create the `device_calendar` account). iOS-only: rejects "not
 *  available on this platform" on Android (no device bridge). */
export const requestDeviceCalendarAccess = async (
  events: boolean,
  reminders: boolean,
): Promise<boolean> => CalFfi.requestDeviceCalendarAccess(events, reminders);

/** Delete an account (unregister adapter + clear secrets + drop row). Rejects
 *  when deleting the implicit local account. */
export const deleteAccount = async (id: string): Promise<void> => {
  await CalFfi.deleteAccount(id);
  scheduleBackgroundPush();
};

/** Rename an account's display name (syncs the change). Rejects on an empty
 *  name / unknown id; returns the updated `Account`. */
export const renameAccount = async (
  id: string,
  newName: string,
): Promise<Account> => {
  const updated = JSON.parse(
    await CalFfi.renameAccountJson(id, newName),
  ) as Account;
  scheduleBackgroundPush();
  return updated;
};

// ── Credential repair ────────────────────────────────────────────────────────

/** External accounts whose required keychain secret is absent — a token that
 *  expired, or a row synced from another device without its device-local
 *  secret. The data behind the "reconnect" banner. (iCal feeds + the local
 *  account are never flagged.) */
export const listAccountsMissingCredentials = async (): Promise<Account[]> =>
  JSON.parse(await CalFfi.listAccountsMissingCredentialsJson()) as Account[];

/** (Re-)enter the secret for a NON-OAuth account — the CalDAV/EWS password or
 *  the Vikunja/Todoist API token — then re-register its adapter so it's live
 *  again without an app restart. Rejects the local account, OAuth accounts
 *  (Google/Microsoft must reconnect via the OAuth flow), and an unknown id. */
export const setAccountSecret = async (
  accountId: string,
  secret: string,
): Promise<void> => {
  await CalFfi.setAccountSecret(accountId, secret);
  scheduleBackgroundPush();
};

// ── Discovery (EWS Autodiscover) ─────────────────────────────────────────────

/** The EWS plugin id the Host drives discovery for. */
const PLUGIN_ID_EWS = 'com.aperio.cal-adapter-ews';

/** Discovered EWS endpoints (the desktop `DiscoveredEndpoints` wire shape). */
export interface DiscoveredEndpoints {
  ews_url: string;
  /** The (possibly redirected) account email Autodiscover resolved. */
  account_email: string;
}

/** Resolve an EWS account's endpoint from email + password via Microsoft
 *  Autodiscover (the desktop "Discover URL" flow). Returns the endpoint +
 *  account email to pre-fill the form; rejects with the plugin's actionable
 *  message on failure (so the UI can fall back to a manually-entered endpoint). */
export const discoverEwsEndpoint = async (
  email: string,
  password: string,
): Promise<DiscoveredEndpoints> =>
  JSON.parse(
    await CalFfi.discoverJson(PLUGIN_ID_EWS, JSON.stringify({ email, password })),
  ) as DiscoveredEndpoints;

// ── OAuth (host-driven two-phase flow around a native auth session) ──────────
// Desktop runs OAuth via the plugin's loopback+browser dance; mobile can't, so
// the Host drives it in two phases and the JS layer opens the consent URL in a
// native auth session. Mirrors the desktop `connect_google_account` /
// `connect_microsoft_account` tail (the Host's complete step does the exchange +
// row + keychain + adapter registration). See `./oauth.ts` for the orchestrator.

/** The statically-embedded plugin ids the Host drives the OAuth flow for. */
export const OAUTH_PLUGIN_IDS: Record<'google' | 'microsoft_graph', string> = {
  google: 'com.aperio.cal-adapter-google',
  microsoft_graph: 'com.aperio.cal-adapter-microsoft-graph',
};

/** The authorize-phase result from {@link beginOauth}: the Host built the consent
 *  URL + PKCE verifier + CSRF state (pure, no network). The caller opens
 *  `authorize_url` in a native auth session and replays `pkce_verifier`/`state`
 *  into {@link completeOauth}. */
export interface OAuthAuthorize {
  authorize_url: string;
  pkce_verifier: string;
  state: string;
}

/** Begin a host-driven OAuth flow. `args` carries the provider's begin inputs —
 *  `{client_id, redirect_uri}` (Google) / `{client_id, authority, redirect_uri}`
 *  (Microsoft). Pure (no network): returns the authorize URL + PKCE/state. */
export const beginOauth = async (
  pluginId: string,
  args: Record<string, string>,
): Promise<OAuthAuthorize> =>
  JSON.parse(
    await CalFfi.beginOauthJson(pluginId, JSON.stringify(args)),
  ) as OAuthAuthorize;

/** The exchange + account-creation inputs for {@link completeOauth} — the desktop
 *  `complete_oauth_json` request shape. `config_json` is the non-secret adapter
 *  config persisted in the row: `{client_id, client_secret}` (Google) /
 *  `{client_id, authority}` (Microsoft). */
export interface CompleteOauthRequest {
  adapter_kind: AdapterKind;
  display_name: string;
  config_json: string;
  client_id: string;
  /** Google only (PKCE public clients like Microsoft omit it). */
  client_secret?: string | null;
  /** Microsoft only — its v2.0 tenant slug; carried through both phases. */
  authority?: string | null;
  code: string;
  pkce_verifier: string;
  state: string;
  returned_state: string;
  redirect_uri: string;
}

/** Complete a host-driven OAuth flow: exchange the redirect's `code` for tokens,
 *  then create + register the account (row + keychain tokens + adapter
 *  registration, all Rust-side). Returns the created `Account`. The exchange hits
 *  the provider's token endpoint. */
export const completeOauth = async (
  pluginId: string,
  request: CompleteOauthRequest,
): Promise<Account> => {
  const created = JSON.parse(
    await CalFfi.completeOauthJson(pluginId, JSON.stringify(request)),
  ) as Account;
  scheduleBackgroundPush();
  return created;
};

/** Re-run OAuth for an EXISTING account (an expired / lost token): exchange the
 *  redirect's `code` for fresh tokens, write them under `accountId`, and
 *  re-register — keeps the account row + its downstream calendar/task/override
 *  references (no remove-and-re-add). `request`'s exchange fields are used; the
 *  kind comes from the existing account. Returns the (unchanged) account. */
export const completeOauthReconnect = async (
  pluginId: string,
  accountId: string,
  request: CompleteOauthRequest,
): Promise<Account> => {
  const account = JSON.parse(
    await CalFfi.completeOauthReconnectJson(pluginId, accountId, JSON.stringify(request)),
  ) as Account;
  scheduleBackgroundPush();
  return account;
};
