import { describe, expect, it } from 'vitest';

import {
  fromContactValues,
  knownLabel,
  primaryChannelValue,
  toContactValues,
} from '@aperio/shared';

describe('contact channels', () => {
  it('reads a bare string as an unlabelled value', () => {
    // Every contact stored before labels existed looks like this on the wire,
    // and the editor must not blow up on — or silently drop — those rows.
    expect(toContactValues(['max@example.com'])).toEqual([
      { value: 'max@example.com', label: null },
    ]);
  });

  it('treats a blank label as no label', () => {
    // "" round-trips out of some providers; a nameless label reads as a
    // stutter to a screen reader, so it becomes an honest absence.
    expect(toContactValues([{ value: 'a@b.example', label: '  ' }])[0].label).toBeNull();
  });

  it('drops rows with no value', () => {
    // An empty row is a UI artefact — the user added it and never typed.
    expect(toContactValues([{ value: '   ', label: 'home' }])).toEqual([]);
  });

  it('writes a label only when there is one', () => {
    expect(
      fromContactValues([
        { value: ' +49 30 111 ', label: ' mobile ' },
        { value: '+49 30 222', label: null },
        { value: '  ', label: 'home' },
      ]),
    ).toEqual([
      { value: '+49 30 111', label: 'mobile' },
      { value: '+49 30 222' },
    ]);
  });

  it('folds provider spellings onto the offered labels', () => {
    // Exchange says "cell", Graph says "business", a German vCard says
    // "privat" — all three are the same picker entry, and a contact that
    // arrives with one must not look like it carries a custom label.
    expect(knownLabel('CELL')).toBe('mobile');
    expect(knownLabel('Business')).toBe('work');
    expect(knownLabel('privat')).toBe('home');
  });

  it('keeps a word it does not know as a custom label', () => {
    expect(knownLabel('Ferienhaus')).toBeNull();
  });

  it('takes the first non-empty value as the primary one', () => {
    expect(primaryChannelValue([{ value: '' }, 'zweite@example.com'])).toBe(
      'zweite@example.com',
    );
    expect(primaryChannelValue([])).toBeNull();
    expect(primaryChannelValue(undefined)).toBeNull();
  });
});
