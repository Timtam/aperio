// Mobile OAuth flow — the host-driven, two-phase PKCE dance wrapped around a
// native auth session (the mobile stand-in for the desktop loopback+browser
// flow). The Rust Host builds the consent URL (begin, pure) and exchanges the
// code + creates the account (complete, network); this module opens that URL in
// an ASWebAuthenticationSession (iOS) / Chrome Custom Tab (Android) and shuttles
// the redirect's `code` + `state` between the two phases.
//
// BYO client-id: the user supplies their own OAuth client (Google Cloud Console
// / Azure portal) at runtime — Aperio bundles no secrets. The redirect URI is a
// custom scheme the user registers on that client.

import * as Linking from 'expo-linking';
import * as WebBrowser from 'expo-web-browser';

import {
  Account,
  AdapterKind,
  OAUTH_PLUGIN_IDS,
  beginOauth,
  completeOauth,
} from './accounts';
import { SYNC_OAUTH_PLUGIN_IDS, completeSyncOauth } from './sync';

/** The custom-scheme redirect the native auth session waits for. Must match the
 *  app's `aperio` scheme (app.json) AND the redirect URI registered on the
 *  user's OAuth client. Kept byte-identical across begin + exchange — the
 *  providers require the exchange redirect to equal the authorize one. */
export const OAUTH_REDIRECT_URI = 'aperio://oauth-callback';

export type OAuthProvider = 'google' | 'microsoft_graph';

/** Normalised connect inputs the user supplies (BYO client-id). */
export interface OAuthConnectInput {
  provider: OAuthProvider;
  displayName: string;
  clientId: string;
  /** Google only — its token endpoint requires the secret even under PKCE. */
  clientSecret?: string;
  /** Microsoft only — the v2.0 tenant slug (`common` / … / a GUID). */
  authority?: string;
}

/** Outcome of {@link connectOAuthAccount}. `cancelled` is the user dismissing the
 *  browser (not an error); failures (bad code, exchange/registration error)
 *  reject so the caller surfaces the message. */
export type OAuthConnectResult =
  | { kind: 'connected'; account: Account }
  | { kind: 'cancelled' };

/** Run the full host-driven OAuth connect: begin → native auth session → parse
 *  the redirect → complete (exchange + create the account). The created account
 *  matches the desktop row exactly (same `adapter_kind` + `config_json` shape). */
export async function connectOAuthAccount(
  input: OAuthConnectInput,
): Promise<OAuthConnectResult> {
  const pluginId = OAUTH_PLUGIN_IDS[input.provider];
  const isMicrosoft = input.provider === 'microsoft_graph';
  const authority = input.authority ?? 'common';

  // 1. begin — pure: the Host builds the consent URL + PKCE verifier + state.
  const beginArgs: Record<string, string> = {
    client_id: input.clientId,
    redirect_uri: OAUTH_REDIRECT_URI,
  };
  if (isMicrosoft) beginArgs.authority = authority;
  const authz = await beginOauth(pluginId, beginArgs);

  // 2. open the consent URL in a native auth session; it resolves once the
  //    provider redirects back to our custom scheme.
  const result = await WebBrowser.openAuthSessionAsync(
    authz.authorize_url,
    OAUTH_REDIRECT_URI,
  );
  if (result.type !== 'success') {
    return { kind: 'cancelled' };
  }

  // 3. parse `code` + `state` (or a provider `error`) from the redirect.
  const params = Linking.parse(result.url).queryParams ?? {};
  const errorParam = firstString(params.error);
  if (errorParam != null) {
    // The user declining consent is a cancellation, not a failure — surface it
    // like a browser dismiss (gentle + localised), not a raw English error token.
    // (consent_required/interaction_required are prompt=none signals, not used
    // here, so they stay genuine errors.)
    if (errorParam === 'access_denied' || errorParam === 'user_cancelled') {
      return { kind: 'cancelled' };
    }
    throw new Error(errorParam);
  }
  const code = firstString(params.code);
  const returnedState = firstString(params.state) ?? '';
  if (code == null || code.length === 0) {
    // A typed sentinel the caller maps to the localised `oauthNoCode` message.
    throw new Error('OAUTH_NO_CODE');
  }

  // 4. complete — exchange the code for tokens, then create + register the
  //    account. config_json is the non-secret row config the registry reads back
  //    (merged with the keychain tokens at registration time, Rust-side).
  const config = isMicrosoft
    ? { client_id: input.clientId, authority }
    : { client_id: input.clientId, client_secret: input.clientSecret ?? '' };

  const account = await completeOauth(pluginId, {
    adapter_kind: input.provider as AdapterKind,
    display_name: input.displayName,
    config_json: JSON.stringify(config),
    client_id: input.clientId,
    client_secret: isMicrosoft ? null : (input.clientSecret ?? ''),
    authority: isMicrosoft ? authority : null,
    code,
    pkce_verifier: authz.pkce_verifier,
    state: authz.state,
    returned_state: returnedState,
    redirect_uri: OAUTH_REDIRECT_URI,
  });
  return { kind: 'connected', account };
}

// ── Sync-target OAuth (Dropbox / Google Drive) ───────────────────────────────
// Same begin → native auth session → complete dance as the account flow, but the
// complete stores the refresh token in the adapter's keychain slot instead of
// creating an account. The caller then calls configureSyncAdapter to activate.

export type SyncOAuthProvider = 'dropbox' | 'googledrive';

export interface SyncOAuthConnectInput {
  provider: SyncOAuthProvider;
  clientId: string;
  /** Dropbox: optional (PKCE public app). Google Drive: required. */
  clientSecret?: string;
}

/** Outcome of {@link connectSyncOAuth}. No account — sync OAuth only stores the
 *  refresh token; activation is a separate `configureSyncAdapter` step. */
export type SyncOAuthResult = { kind: 'connected' } | { kind: 'cancelled' };

/** Run the host-driven OAuth for a sync target: begin → native auth session →
 *  complete (exchange + store the refresh token). Does NOT activate the target —
 *  follow with `configureSyncAdapter({kind, client_id, …})`. */
export async function connectSyncOAuth(
  input: SyncOAuthConnectInput,
): Promise<SyncOAuthResult> {
  const pluginId = SYNC_OAUTH_PLUGIN_IDS[input.provider];

  // 1. begin — pure: the Host builds the consent URL + PKCE verifier + state.
  const authz = await beginOauth(pluginId, {
    client_id: input.clientId,
    redirect_uri: OAUTH_REDIRECT_URI,
  });

  // 2. open the consent URL in a native auth session.
  const result = await WebBrowser.openAuthSessionAsync(
    authz.authorize_url,
    OAUTH_REDIRECT_URI,
  );
  if (result.type !== 'success') {
    return { kind: 'cancelled' };
  }

  // 3. parse code + state (or a provider error) from the redirect.
  const params = Linking.parse(result.url).queryParams ?? {};
  const errorParam = firstString(params.error);
  if (errorParam != null) {
    if (errorParam === 'access_denied' || errorParam === 'user_cancelled') {
      return { kind: 'cancelled' };
    }
    throw new Error(errorParam);
  }
  const code = firstString(params.code);
  const returnedState = firstString(params.state) ?? '';
  if (code == null || code.length === 0) {
    throw new Error('OAUTH_NO_CODE');
  }

  // 4. complete — exchange the code + store the refresh token in the keychain.
  await completeSyncOauth(pluginId, {
    client_id: input.clientId,
    client_secret: input.clientSecret ?? null,
    code,
    pkce_verifier: authz.pkce_verifier,
    state: authz.state,
    returned_state: returnedState,
    redirect_uri: OAUTH_REDIRECT_URI,
  });
  return { kind: 'connected' };
}

/** Expo's `Linking.parse` types query values as `string | string[] | undefined`
 *  (a key can repeat); take the first for the single-valued OAuth params. */
function firstString(
  value: string | string[] | undefined,
): string | undefined {
  if (value == null) return undefined;
  return Array.isArray(value) ? value[0] : value;
}
