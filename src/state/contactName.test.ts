import { describe, expect, it } from 'vitest';

import { deriveDisplayName } from '@aperio/shared';

// The composition both editors keep the display name in step with while the
// user hasn't typed their own (the Outlook shape — see shared/contactName.ts).
describe('deriveDisplayName', () => {
  it('composes prefix, given, family and suffix in order', () => {
    expect(
      deriveDisplayName({
        namePrefix: 'Prof. Dr.',
        givenName: 'Max',
        familyName: 'Mustermann',
        nameSuffix: 'jun.',
      }),
    ).toBe('Prof. Dr. Max Mustermann jun.');
  });

  it('skips empty parts without doubling spaces', () => {
    expect(
      deriveDisplayName({ givenName: 'Max', familyName: 'Mustermann' }),
    ).toBe('Max Mustermann');
    expect(
      deriveDisplayName({ namePrefix: 'Dr.', familyName: 'Mustermann' }),
    ).toBe('Dr. Mustermann');
  });

  it('trims each part — a space-padded field is not a part', () => {
    expect(
      deriveDisplayName({ givenName: '  Max ', familyName: ' Mustermann ' }),
    ).toBe('Max Mustermann');
    expect(deriveDisplayName({ givenName: '   ' })).toBe('');
  });

  it('falls back to the organization when there are no name parts', () => {
    // A company contact shows its company — the same fallback Apple and the
    // adapters' read-side display-name assembly apply.
    expect(deriveDisplayName({ organization: 'Example GmbH' })).toBe(
      'Example GmbH',
    );
    // …but any name part outranks it.
    expect(
      deriveDisplayName({ familyName: 'Mustermann', organization: 'Example' }),
    ).toBe('Mustermann');
  });

  it('is empty when nothing is filled in', () => {
    expect(deriveDisplayName({})).toBe('');
    expect(deriveDisplayName({ namePrefix: null, givenName: null })).toBe('');
  });
});
