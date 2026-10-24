import { describe, expect, it } from 'vitest';

import { carvedOccurrenceKeptSentence, originalKeptSentence } from '@aperio/shared';

import i18n from '../i18n';

const de = i18n.getFixedT('de');
const en = i18n.getFixedT('en');
const names: Record<string, string> = { L1: 'Haushalt' };
const nameOf = (id: string) => names[id];

/**
 * A move whose original the provider was too busy to delete (decision 277):
 * the move stands, and the sentence says where the original still is and that
 * moving it again would make a second copy.
 */
describe('originalKeptSentence', () => {
  it('says nothing when every original went', () => {
    expect(originalKeptSentence([{ title: 'Einkaufen' }], nameOf, de)).toBeNull();
    expect(
      originalKeptSentence([{ title: 'Einkaufen', original_left_in: null }], nameOf, de),
    ).toBeNull();
  });

  it('names the one original and the list it stayed in', () => {
    const rows = [{ title: 'Einkaufen', original_left_in: 'L1' }, { title: 'Milch' }];
    expect(originalKeptSentence(rows, nameOf, de)).toBe(
      '„Einkaufen“ wurde verschoben, aber das Original in „Haushalt“ ließ sich nicht entfernen, weil der Anbieter gerade ausgelastet ist. Lösche es dort selbst; verschiebe es nicht noch einmal.',
    );
    expect(originalKeptSentence(rows, nameOf, en)).toBe(
      '"Einkaufen" was moved, but its original in "Haushalt" could not be removed because the provider is busy right now. Delete it there yourself; do not move it again.',
    );
  });

  it('names every original that stayed, a subtask among them', () => {
    const rows = [
      { title: 'Einkaufen', original_left_in: 'L1' },
      { title: 'Milch', original_left_in: 'L1' },
      { title: 'Brot', original_left_in: 'L1' },
    ];
    expect(originalKeptSentence(rows, nameOf, de)).toBe(
      'Verschoben, aber diese Originale in „Haushalt“ ließen sich nicht entfernen, weil der Anbieter gerade ausgelastet ist: „Einkaufen“, „Milch“ und „Brot“. Lösche sie dort selbst; verschiebe sie nicht noch einmal.',
    );
  });

  it('falls back to the id of a list it cannot name', () => {
    expect(
      originalKeptSentence([{ title: 'Einkaufen', original_left_in: 'L9' }], nameOf, en),
    ).toContain('"L9"');
  });
});

describe('carvedOccurrenceKeptSentence', () => {
  const calendarName = (id: string) => ({ work: 'Arbeit', home: 'Privat' })[id];
  it('says a carve-out kept in its own calendar as an edit, not a move', () => {
    expect(carvedOccurrenceKeptSentence('Planung', 'work', 'work', calendarName, de)).toBe(
      '„Planung“ ist als eigener Termin gespeichert, aber der Termin der Serie in „Arbeit“ ließ sich nicht auslassen, weil der Anbieter gerade ausgelastet ist. Lösche diesen Termin der Serie dort selbst; speichere nicht noch einmal.',
    );
  });

  it('says one saved to another calendar as a move whose original stayed', () => {
    expect(carvedOccurrenceKeptSentence('Planung', 'work', 'home', calendarName, de)).toContain(
      'wurde verschoben, aber das Original in „Arbeit“',
    );
  });
});
