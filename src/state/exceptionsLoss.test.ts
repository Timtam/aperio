import { describe, expect, it } from 'vitest';

import {
  deletionsNotRestoredSentence,
  exceptionsLossQuestion,
  parseExceptionsLoss,
} from '@aperio/shared';
import i18n from '../i18n';

/**
 * The question both editors ask before a save makes Exchange drop occurrences
 * of a series the user changed or deleted on their own, and the sentence that
 * names deleted ones that came back (decisions 243-253).
 */

const de = (key: string, values?: Record<string, unknown>): string =>
  i18n.getFixedT('de')(key, values as never) as unknown as string;
const en = (key: string, values?: Record<string, unknown>): string =>
  i18n.getFixedT('en')(key, values as never) as unknown as string;

describe('the refusal detail', () => {
  it('reads as the core writes it, and nothing else', () => {
    // cal_core::SeriesRewrite::detail, `{rewrite}:{changed}:{deleted}`.
    expect(parseExceptionsLoss('slot:2:1')).toEqual({ rewrite: 'slot', changed: 2, deleted: 1 });
    expect(parseExceptionsLoss(' pattern:0:3 ')).toEqual({
      rewrite: 'pattern',
      changed: 0,
      deleted: 3,
    });
    for (const odd of ['zone', 'slot:3', 'slot:x:0', 'slot:1:0:9', 'moved:1:0', 'slot:-1:0']) {
      expect(parseExceptionsLoss(odd)).toBeNull();
    }
  });
});

describe('the question', () => {
  it('says what the save rewrites and what that loses, in German and in English', () => {
    expect(exceptionsLossQuestion({ rewrite: 'slot', changed: 2, deleted: 0 }, 'Teamrunde', de)).toEqual({
      title: 'Vorkommen gehen verloren',
      message:
        '„Teamrunde“: Diese Änderung schreibt Beginn und Ende der Serie neu. Exchange verwirft ' +
        'dabei 2 Vorkommen, die einzeln geändert wurden: Sie nehmen wieder die Angaben der Serie ' +
        'an. Trotzdem speichern?',
      confirm: 'Trotzdem speichern',
    });
    expect(
      exceptionsLossQuestion({ rewrite: 'pattern', changed: 1, deleted: 2 }, 'Teamrunde', de).message,
    ).toBe(
      '„Teamrunde“: Diese Änderung schreibt das Muster der Wiederholung neu. Exchange verwirft ' +
        'dabei ein einzeln geändertes Vorkommen und 2 gelöschte Vorkommen: Geänderte nehmen ' +
        'wieder die Angaben der Serie an, gelöschte erscheinen wieder. Trotzdem speichern?',
    );
    expect(
      exceptionsLossQuestion({ rewrite: 'slot', changed: 0, deleted: 1 }, 'Team', en).message,
    ).toBe(
      '"Team": This change writes the series\' start and end again. Exchange then drops one ' +
        'deleted occurrence: it comes back. Save anyway?',
    );
  });
});

describe('deleted occurrences that came back', () => {
  it('are named by their days, and nothing is said when there are none', () => {
    expect(deletionsNotRestoredSentence(undefined, 'Teamrunde', 'de', de)).toBeNull();
    expect(deletionsNotRestoredSentence([], 'Teamrunde', 'de', de)).toBeNull();
    expect(
      deletionsNotRestoredSentence(['2026-11-16T09:00:00Z'], 'Teamrunde', 'de', de),
    ).toBe(
      '„Teamrunde“ ist gespeichert. Ein gelöschtes Vorkommen hat Exchange dabei zurückgebracht, ' +
        'und Aperio konnte es nicht wieder löschen: 16. November 2026. Lösche es bitte einzeln.',
    );
    expect(
      deletionsNotRestoredSentence(
        ['2026-11-16T09:00:00Z', '2026-11-23T09:00:00Z'],
        'Teamrunde',
        'de',
        de,
      ),
    ).toMatch(/2 gelöschte Vorkommen .*: 16\. November 2026 und 23\. November 2026\./);
  });
});
