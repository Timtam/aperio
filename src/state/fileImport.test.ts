import { describe, expect, it } from 'vitest';
import {
  importedForEditor,
  importFailureSentence,
  skippedSentence,
  type ImportedEvent,
} from '@aperio/shared';

import i18n from '../i18n';

/**
 * The file-import rules both shells share (`shared/fileImport.ts`): which
 * appointment the editor gets, and how what was left out, or what failed, is
 * said.
 */

const t = (key: string, values?: Record<string, unknown>) => i18n.t(key, values);

const invitation: ImportedEvent = {
  title: 'Projektbesprechung',
  start: '2026-11-18T13:00:00Z',
  end: '2026-11-18T14:00:00Z',
  all_day: false,
  reminders: [],
  attendees: ['Max <max@example.org>'],
  organizer: 'erika@example.org',
};

describe('importedForEditor', () => {
  it('sets an invitation aside until each provider keeps it quietly', () => {
    const { event, invitationSetAside } = importedForEditor(invitation);
    expect(invitationSetAside).toBe(true);
    expect(event.attendees).toEqual([]);
    expect('organizer' in event).toBe(false);
    expect(event.title).toBe('Projektbesprechung');
  });

  it('keeps the invitees of an appointment without an organizer', () => {
    const own = { ...invitation, organizer: undefined };
    const { event, invitationSetAside } = importedForEditor(own);
    expect(invitationSetAside).toBe(false);
    expect(event.attendees).toEqual(['Max <max@example.org>']);
  });
});

describe('skippedSentence', () => {
  it('names each part by its kind and title, never a count', () => {
    expect(
      skippedSentence(
        [
          { kind: 'task', title: 'Geschenke besorgen' },
          { kind: 'journal' },
          { kind: 'reply', title: 'Angenommen: Budget' },
        ],
        t,
      ),
    ).toBe(
      'Nicht übernommen: die Aufgabe „Geschenke besorgen“, eine Notiz ohne Titel und die ' +
        'Antwort „Angenommen: Budget“.',
    );
    expect(skippedSentence([], t)).toBeNull();
  });
});

describe('importFailureSentence', () => {
  it('says each kind of failure in words, never the plugin’s English', () => {
    const said = (err: unknown) => importFailureSentence('a.ics', err, t);
    expect(said({ code: 'unsupported', message: 'no import format reads a.ics' })).toBe(
      '„a.ics“ ließ sich nicht importieren: Aperio kann Dateien dieser Art nicht importieren.',
    );
    expect(said({ code: 'invalid_input', message: 'import-too-large: a.ics is 30 bytes' })).toBe(
      '„a.ics“ ließ sich nicht importieren: Die Datei ist größer als 25 MB.',
    );
    expect(said({ code: 'invalid_input', message: 'the file holds no calendar' })).toBe(
      '„a.ics“ ließ sich nicht importieren: Die Datei ist keine Kalenderdatei oder beschädigt. ' +
        'Einzelheiten stehen im Protokoll.',
    );
    expect(said({ code: 'internal', message: 'could not open a.ics' })).toBe(
      '„a.ics“ ließ sich nicht importieren: Die Datei ließ sich nicht öffnen. Einzelheiten ' +
        'stehen im Protokoll.',
    );
  });
});
