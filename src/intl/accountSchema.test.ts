import { describe, expect, it } from 'vitest';

import {
  collectValues,
  firstMissingField,
  type AccountFormField,
  type AccountFormSpec,
} from '@aperio/shared';

function field(
  key: string,
  overrides: Partial<AccountFormField> = {},
): AccountFormField {
  return {
    key,
    kind: 'text',
    label: key,
    hint: null,
    required: false,
    default_bool: null,
    default_text: null,
    ...overrides,
  };
}

/** The shape an adapter with its own OAuth publishes. */
const OAUTH_SPEC: AccountFormSpec = {
  plugin_id: 'com.example.thing',
  fields: [
    field('client_id', { required: true }),
    field('client_secret', { kind: 'secret', required: true }),
    field('site_url'),
    field('use_personal_room', { kind: 'bool', default_bool: false }),
  ],
  oauth: {
    builtin: false,
    client_id_field: 'client_id',
    client_secret_field: 'client_secret',
  },
  owns_containers: false,
};

const BASIC_SPEC: AccountFormSpec = {
  plugin_id: 'com.example.dav',
  fields: [
    field('server_url', { kind: 'url', required: true }),
    field('username', { required: true }),
    field('password', { kind: 'secret', required: true }),
  ],
  oauth: null,
  owns_containers: true,
};

describe('firstMissingField', () => {
  it('names the first required field left blank', () => {
    const missing = firstMissingField(BASIC_SPEC, {
      server_url: 'https://dav.test/',
    });
    expect(missing?.key).toBe('username');
  });

  it('treats whitespace as blank', () => {
    const missing = firstMissingField(BASIC_SPEC, {
      server_url: '   ',
      username: 'toni',
      password: 'pw',
    });
    expect(missing?.key).toBe('server_url');
  });

  it('is satisfied by a complete form', () => {
    expect(
      firstMissingField(BASIC_SPEC, {
        server_url: 'https://dav.test/',
        username: 'toni',
        password: 'pw',
      }),
    ).toBeNull();
  });

  it('never demands a checkbox', () => {
    // A checkbox always has a value; "required" on one would be unsatisfiable
    // in the off position.
    const spec: AccountFormSpec = {
      ...BASIC_SPEC,
      fields: [field('agree', { kind: 'bool', required: true })],
    };
    expect(firstMissingField(spec, {})).toBeNull();
  });

  it('still demands the OAuth pair when the build carries no credentials', () => {
    expect(firstMissingField(OAUTH_SPEC, {})?.key).toBe('client_id');
  });

  it('demands neither half when the build carries credentials', () => {
    // Leaving both blank is how the user says "sign in with what Aperio has".
    const spec: AccountFormSpec = {
      ...OAUTH_SPEC,
      oauth: { ...OAUTH_SPEC.oauth!, builtin: true },
    };
    expect(firstMissingField(spec, {})).toBeNull();
  });

  it('accepts a value that came from the declared default', () => {
    const spec: AccountFormSpec = {
      ...BASIC_SPEC,
      fields: [
        field('server_url', { required: true, default_text: 'https://a.test/' }),
      ],
    };
    expect(firstMissingField(spec, {})).toBeNull();
  });
});

describe('collectValues', () => {
  it('trims what it keeps', () => {
    const out = collectValues(BASIC_SPEC, {
      server_url: '  https://dav.test/  ',
      username: 'toni',
      password: 'pw',
    });
    expect(out.server_url).toBe('https://dav.test/');
  });

  it('drops a blank optional field rather than sending an empty string', () => {
    // "the user did not say" and "the user said nothing" are different answers.
    // Webex's site field is exactly this: blank means "use the account's own
    // default site", where "" would mean a site whose name is nothing.
    const out = collectValues(OAUTH_SPEC, {
      client_id: 'C-mine',
      client_secret: 's3cr3t',
      site_url: '   ',
    });
    expect('site_url' in out).toBe(false);
  });

  it('always sends a checkbox, defaulting to what the adapter declared', () => {
    const out = collectValues(OAUTH_SPEC, {});
    expect(out.use_personal_room).toBe(false);
    const on = collectValues(OAUTH_SPEC, { use_personal_room: true });
    expect(on.use_personal_room).toBe(true);
  });

  it('sends the credential pair like any other field', () => {
    // The backend, not the form, decides what a pair means — the form's job is
    // to report exactly what was typed.
    const out = collectValues(OAUTH_SPEC, {
      client_id: 'C-mine',
      client_secret: 's3cr3t',
    });
    expect(out.client_id).toBe('C-mine');
    expect(out.client_secret).toBe('s3cr3t');
  });

  it('sends nothing for a pair the user left blank', () => {
    const out = collectValues(OAUTH_SPEC, {});
    expect('client_id' in out).toBe(false);
    expect('client_secret' in out).toBe(false);
  });
});
