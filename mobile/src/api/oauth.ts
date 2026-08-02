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
  beginAccountOauth,
  beginAccountReconnect,
  completeAccountReconnect,
  connectAccount,
} from './accounts';

/** The custom-scheme redirect the native auth session waits for. Must match the
 *  app's `aperio` scheme (app.json) AND the redirect URI registered on the
 *  user's OAuth client. Kept byte-identical across begin + exchange — the
 *  providers require the exchange redirect to equal the authorize one. */
export const OAUTH_REDIRECT_URI = 'aperio://oauth-callback';

/** Outcome of {@link connectOAuthAccount}. `cancelled` is the user dismissing the
 *  browser (not an error); failures (bad code, exchange/registration error)
 *  reject so the caller surfaces the message. */
export type OAuthConnectResult =
  | { kind: 'connected'; account: Account }
  | { kind: 'cancelled' };

/** Re-run the provider sign-in for an EXISTING account whose grant expired.
 *
 *  Nothing here names a provider. The host reads the account's own stored
 *  client — including the secret it kept in the keychain, or the build's own
 *  registration when the account was connected that way — builds the consent
 *  URL, and writes the fresh tokens back under the same account id, so its
 *  calendars, colours and overrides survive.
 *
 *  The version this replaces derived the provider from the adapter kind with a
 *  two-way branch that fell back to Google, so a Webex account whose grant
 *  expired would have been sent to Google's endpoint. */
export async function reconnectOAuthAccount(
  account: Account,
): Promise<OAuthConnectResult> {
  const authz = await beginAccountReconnect(account.id);
  const result = await WebBrowser.openAuthSessionAsync(
    authz.authorize_url,
    OAUTH_REDIRECT_URI,
  );
  if (result.type !== 'success') {
    return { kind: 'cancelled' };
  }

  const params = Linking.parse(result.url).queryParams ?? {};
  const errorParam = firstString(params.error);
  if (errorParam != null) {
    if (errorParam === 'access_denied' || errorParam === 'user_cancelled') {
      return { kind: 'cancelled' };
    }
    throw new Error(errorParam);
  }
  const code = firstString(params.code);
  if (code == null || code.length === 0) {
    throw new Error('OAUTH_NO_CODE');
  }

  const reconnected = await completeAccountReconnect(account.id, {
    code,
    pkce_verifier: authz.pkce_verifier,
    state: authz.state,
    returned_state: firstString(params.state) ?? '',
  });
  return { kind: 'connected', account: reconnected };
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

/** Expo's `Linking.parse` types query values as `string | string[] | undefined`
 *  (a key can repeat); take the first for the single-valued OAuth params. */
function firstString(
  value: string | string[] | undefined,
): string | undefined {
  if (value == null) return undefined;
  return Array.isArray(value) ? value[0] : value;
}

// ── Schema-driven connect ───────────────────────────────────────────────────

/** Run a connect for any adapter that declares an account schema.
 *
 *  One function for both shapes. An adapter with an OAuth block gets the
 *  two-phase dance around a native auth session; one without goes straight to
 *  the account creation. Which it is comes from the adapter's own declaration,
 *  so this never grows a branch per provider — the whole point of the schema.
 *
 *  The redirect URI is the app's own scheme, which is the only thing a native
 *  auth session can return to. The adapter declares it (defaulting to
 *  `aperio://oauth-callback`) so a plugin registered against a different scheme
 *  can say so. */
export async function connectSchemaAccount(input: {
  kind: AdapterKind;
  displayName: string;
  values: Record<string, string | boolean>;
  /** From the adapter's spec: absent means no sign-in step. */
  hasOauth: boolean;
}): Promise<OAuthConnectResult> {
  if (!input.hasOauth) {
    const account = await connectAccount({
      adapter_kind: input.kind,
      display_name: input.displayName,
      values: input.values,
    });
    return { kind: 'connected', account };
  }

  const authz = await beginAccountOauth(input.kind, input.values);
  const result = await WebBrowser.openAuthSessionAsync(
    authz.authorize_url,
    OAUTH_REDIRECT_URI,
  );
  if (result.type !== 'success') {
    return { kind: 'cancelled' };
  }

  const params = Linking.parse(result.url).queryParams ?? {};
  const errorParam = firstString(params.error);
  if (errorParam != null) {
    // Declining consent is a cancellation, not a failure — the same reading the
    // Google/Microsoft flow above gives it.
    if (errorParam === 'access_denied' || errorParam === 'user_cancelled') {
      return { kind: 'cancelled' };
    }
    throw new Error(errorParam);
  }
  const code = firstString(params.code);
  if (code == null || code.length === 0) {
    throw new Error('OAUTH_NO_CODE');
  }

  const account = await connectAccount({
    adapter_kind: input.kind,
    display_name: input.displayName,
    // The same values go back: the host re-derives which OAuth client this is
    // rather than holding one across the round trip.
    values: input.values,
    code,
    pkce_verifier: authz.pkce_verifier,
    state: authz.state,
    returned_state: firstString(params.state) ?? '',
  });
  return { kind: 'connected', account };
}
