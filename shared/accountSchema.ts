/**
 * An adapter's connect form, as its `plugin.json` declares it — and the pure
 * logic both frontends run over it.
 *
 * Shared because the desktop and the mobile app must agree on what "the user
 * left this blank" means. They ask different hosts (a Tauri command / the
 * cal-ffi bridge) but both hosts answer with this shape, and both frontends
 * render it without knowing what any field means.
 *
 * The SHAPES are no longer written here. `host_core::account_form` builds the
 * spec once for both hosts and `cargo xtask ts-types` generates the
 * declarations from it, so this file is the door and the logic below it.
 *
 * That is not tidiness. The two hosts used to build the spec separately, and
 * they had drifted: the mobile one emitted no `options`, so
 * `AccountSchemaForm` mapped over `undefined` and took the form down on the FTP
 * and SFTP plugins — both of which declare a `choice` field. The hand-written
 * type said `options` was there, the mobile side parses the bridge's answer
 * with a bare cast, and nothing in between could notice.
 */
export type { AccountFieldKind } from './generated/AccountFieldKind';
export type { AccountFormOption } from './generated/AccountFormOption';
export type { AccountFormField } from './generated/AccountFormField';
export type { AccountFormRequirement } from './generated/AccountFormRequirement';
export type { AccountFormAction } from './generated/AccountFormAction';
export type { AccountFormOauth } from './generated/AccountFormOauth';
export type { AccountFormSpec } from './generated/AccountFormSpec';

import type { AccountFormField } from './generated/AccountFormField';
import type { AccountFormSpec } from './generated/AccountFormSpec';

/** Which fields the OAuth posture makes optional, if any. */
function optionalUnderBuiltinCredentials(spec: AccountFormSpec): Set<string> {
  const optional = new Set<string>();
  if (spec.oauth?.builtin) {
    optional.add(spec.oauth.client_id_field);
    if (spec.oauth.client_secret_field) {
      optional.add(spec.oauth.client_secret_field);
    }
  }
  return optional;
}

/** The effective text of a field: what was typed, else its declared default. */
function textOf(
  field: AccountFormField,
  values: Record<string, string | boolean>,
): string {
  const value = values[field.key];
  return typeof value === 'string' ? value : (field.default_text ?? '');
}

/**
 * The first required field still empty, or `null` when the form is complete.
 *
 * The OAuth client pair is the one exception a schema cannot express on its
 * own: with built-in credentials both halves are optional, without them both
 * are required. Half a pair is refused by the backend rather than quietly
 * completed — see `choose_oauth_client` — so this only catches the plain
 * "nothing entered" case, where the message can name a field the user is
 * looking at.
 */
export function firstMissingField(
  spec: AccountFormSpec,
  values: Record<string, string | boolean>,
): AccountFormField | null {
  const optional = optionalUnderBuiltinCredentials(spec);
  for (const field of spec.fields) {
    if (field.kind === 'bool' || !field.required || optional.has(field.key)) {
      continue;
    }
    if (!textOf(field, values).trim()) return field;
  }
  return null;
}

/**
 * The values to send, dropping anything left blank.
 *
 * An untouched optional field has to stay ABSENT rather than become `""`: the
 * two mean different things to an adapter. Webex's site field is exactly that
 * case — blank means "use the account's own default site", where an empty
 * string would mean "a site whose name is nothing".
 */
export function collectValues(
  spec: AccountFormSpec,
  values: Record<string, string | boolean>,
): Record<string, string | boolean> {
  const out: Record<string, string | boolean> = {};
  for (const field of spec.fields) {
    if (field.kind === 'bool') {
      const value = values[field.key];
      out[field.key] =
        typeof value === 'boolean' ? value : (field.default_bool ?? false);
      continue;
    }
    const text = textOf(field, values).trim();
    if (text) out[field.key] = text;
  }
  return out;
}
