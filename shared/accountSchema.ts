/**
 * An adapter's connect form, as its `plugin.json` declares it — and the pure
 * logic both frontends run over it.
 *
 * Shared because the desktop and the mobile app must agree on what "the user
 * left this blank" means. They ask different hosts (a Tauri command / the
 * cal-ffi bridge) but both hosts answer with this shape, and both frontends
 * render it without knowing what any field means.
 */

/** One thing the connect form asks for, or one setting it offers. */
/** One entry of a `choice` field. */
export interface AccountFormOption {
  value: string;
  /** Already in the reader's language, like every other label here. */
  label: string;
}

export interface AccountFormField {
  key: string;
  /**
   * `choice` is a closed set the adapter declares — several adapters do NOT
   * reject a value outside their set (the FTPS plugin falls through to
   * explicit), so a free-text box would let a typo pick a different transport
   * in silence.
   *
   * `directory` and `file` are paths on THIS machine. They differ only in which
   * picker to open, and where there is none they are a plain text field.
   *
   * `number` still travels as a string — every control here produces text —
   * and the host converts it before the adapter sees it. What the kind buys is
   * the right control: a numeric keyboard on the phone, and a spinner rather
   * than a text box on the desktop.
   */
  kind:
    | 'text'
    | 'url'
    | 'secret'
    | 'bool'
    | 'choice'
    | 'directory'
    | 'file'
    | 'number';
  /**
   * Already in the reader's language.
   *
   * The host resolved it against the PLUGIN's own catalogue before sending it,
   * so there is nothing here to translate: the app carries no word about
   * somebody else's provider, and a third-party adapter with no catalogue
   * arrives as the literal its author wrote — which beats a missing-key marker.
   */
  label: string;
  hint: string | null;
  required: boolean;
  default_bool: boolean | null;
  default_text: string | null;
  /** The choices, for `kind === 'choice'`. Empty otherwise. */
  options: AccountFormOption[];
  /**
   * Whether this value means anything only on the device that entered it — a
   * filesystem path, typically.
   *
   * Only the adapter can answer it: a host cannot tell a path from a URL by
   * looking, and guessing wrong either makes the user retype settings on every
   * device or lets one machine's paths overwrite another's.
   */
  device_local: boolean;
}

/**
 * A button the adapter's form offers besides "add" — a lookup it can do for the
 * user, like asking Autodiscover for an Exchange endpoint.
 *
 * Everything here arrives in the reader's language and names no adapter: the
 * host resolved it from the manifest, and the frontend renders a button per
 * entry without knowing what any of them do.
 */
export interface AccountFormAction {
  key: string;
  label: string;
  /** Shown while it runs, so the button says it is working. */
  busy_label: string | null;
  /** Announced when it succeeds. */
  success: string | null;
  /** Description the button points at, for saying what will happen first. */
  hint: string | null;
  /** Fields that must be filled, each with what to say when it is not. */
  requires: { field: string; message: string }[];
}

/** The OAuth half, when the adapter signs in that way. */
export interface AccountFormOauth {
  /** True when this build carries credentials for the provider, so the two
   *  client fields need not be shown or filled at all. */
  builtin: boolean;
  client_id_field: string;
  client_secret_field: string | null;
}

/** Everything needed to render an adapter's connect form. */
export interface AccountFormSpec {
  plugin_id: string;
  fields: AccountFormField[];
  actions?: AccountFormAction[];
  oauth: AccountFormOauth | null;
  /** Whether accounts of this adapter own calendars and task lists. False for a
   *  videoconference adapter, which owns neither — so a frontend can skip the
   *  catalog refresh after connecting one without keeping its own list of which
   *  adapters those are. */
  owns_containers: boolean;
}

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
