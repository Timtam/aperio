import { describe, expect, it } from 'vitest';
import {
  alreadyHeld,
  heldSentence,
  heldStatusSentence,
  importedCreate,
  importEditorPeople,
  importFailureSentence,
  importInto,
  importIntoSentence,
  importOwner,
  importPeople,
  importRowLabel,
  importSpan,
  importWhen,
  invitationListSentence,
  rechosenForHeld,
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

describe('importOwner', () => {
  it('tells nobody, the account and someone else apart, and waits for the address', () => {
    const { organizer: _organizer, ...noOrganizer } = invitation;
    expect(importOwner(noOrganizer, 'toni@example.org')).toBe('none');
    expect(importOwner(invitation, undefined)).toBe('checking');
    expect(importOwner(invitation, 'MAILTO:Erika@Example.org')).toBe('me');
    expect(importOwner(invitation, 'toni@example.org')).toBe('someone-else');
    expect(importOwner(invitation, null)).toBe('someone-else');
  });
});

describe('importInto', () => {
  it('keeps an invitation where the calendar keeps invitations, and sets it aside elsewhere', () => {
    expect(importInto('someone-else', { invitation_import: 'kept' })).toEqual({
      kind: 'invitation',
      tentative: false,
    });
    expect(importInto('someone-else', { invitation_import: 'kept-tentative' })).toEqual({
      kind: 'invitation',
      tentative: true,
    });
    expect(importInto('someone-else', { invitation_import: 'not-kept' })).toEqual({
      kind: 'set-aside',
    });
    // A listing from before the flag existed keeps nothing.
    expect(importInto('someone-else', {})).toEqual({ kind: 'set-aside' });
    expect(importInto('me', { invitation_import: 'not-kept' })).toEqual({ kind: 'new' });
    expect(importInto('none', { invitation_import: 'kept' })).toEqual({ kind: 'new' });
    expect(importInto('checking', { invitation_import: 'kept' })).toEqual({ kind: 'checking' });
  });
});

describe('importPeople', () => {
  it('writes an invitation as someone else’s, a set-aside one without people, an own one as new', () => {
    expect(importPeople(invitation, { kind: 'invitation', tentative: false })).toEqual({
      attendees: ['Max <max@example.org>'],
      organizer: 'erika@example.org',
      organized_elsewhere: true,
      keeps_invitation: true,
    });
    expect(importPeople(invitation, { kind: 'set-aside' })).toEqual({
      attendees: [],
      organizer: null,
      organized_elsewhere: false,
      keeps_invitation: false,
    });
    // The account's own meeting names itself as organizer, so the host keeps
    // the account out of its invitees.
    expect(importPeople(invitation, { kind: 'new' })).toEqual({
      attendees: ['Max <max@example.org>'],
      organizer: 'erika@example.org',
      organized_elsewhere: false,
      keeps_invitation: false,
    });
  });
});

describe('importIntoSentence', () => {
  it('says what happens to an invitation in the calendar, and nothing for a new one', () => {
    expect(importIntoSentence(invitation, { kind: 'invitation', tentative: false }, 'Arbeit', t)).toBe(
      'Eine Einladung von erika@example.org: Organisator und Teilnehmer bleiben, und niemand wird benachrichtigt.',
    );
    expect(importIntoSentence(invitation, { kind: 'invitation', tentative: true }, 'Arbeit', t)).toMatch(
      /Deine Antwort steht danach auf „Mit Vorbehalt“/,
    );
    expect(importIntoSentence(invitation, { kind: 'set-aside' }, 'Arbeit', t)).toBe(
      'Eine Einladung von erika@example.org: In „Arbeit“ lassen sich Organisator und Teilnehmer ' +
        'nicht übernehmen, ohne dass jemand benachrichtigt wird. Der Termin wird deshalb dein ' +
        'eigener, ohne Teilnehmer.',
    );
    expect(importIntoSentence(invitation, { kind: 'new' }, 'Arbeit', t)).toBeNull();
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

describe('importSpan', () => {
  it('runs from the earliest start to the latest end, a minute wider', () => {
    const at = (start: string, end: string): ImportedEvent => ({ ...invitation, start, end });
    expect(
      importSpan([
        at('2026-11-18T13:00:00Z', '2026-11-18T14:00:00Z'),
        // An offset is read as the instant it is, never compared as text.
        at('2026-11-18T10:00:00+01:00', '2026-11-18T10:00:00+01:00'),
      ]),
    ).toEqual({ start: '2026-11-18T08:59:00.000Z', end: '2026-11-18T14:01:00.000Z' });
    expect(importSpan([])).toBeNull();
  });
});

describe('alreadyHeld', () => {
  it('finds what the calendar holds by the UID, and nothing without one', () => {
    const events: ImportedEvent[] = [
      { ...invitation, uid: 'kept@example.org' },
      { ...invitation, uid: 'new@example.org' },
      { ...invitation },
      { ...invitation, uid: '  ' },
    ];
    const rows = [{ ical_uid: ' kept@example.org ' }, { ical_uid: null }, {}, { ical_uid: '  ' }];
    expect([...alreadyHeld(events, rows)]).toEqual([0]);
  });
});

describe('importEditorPeople', () => {
  const form = ['Max <max@example.org>', 'neu@example.org'];
  it('writes a kept invitation as the file has it, whatever the form holds', () => {
    expect(
      importEditorPeople(invitation, { kind: 'invitation', tentative: true }, form),
    ).toEqual({
      attendees: ['Max <max@example.org>'],
      organizer: 'erika@example.org',
      organized_elsewhere: true,
      keeps_invitation: true,
    });
  });
  it('carries the form, and names the organizer of the account’s own meeting', () => {
    expect(importEditorPeople(invitation, { kind: 'new' }, form)).toEqual({
      attendees: form,
      organizer: 'erika@example.org',
      organized_elsewhere: false,
      keeps_invitation: false,
    });
    expect(importEditorPeople(invitation, { kind: 'set-aside' }, [])).toEqual({
      attendees: [],
      organizer: null,
      organized_elsewhere: false,
      keeps_invitation: false,
    });
    expect(importEditorPeople(null, null, form)).toEqual({
      attendees: form,
      organizer: null,
      organized_elsewhere: false,
      keeps_invitation: false,
    });
  });
});

describe('rechosenForHeld', () => {
  it('unchecks what the calendar holds and checks again what only the one before held', () => {
    // Rows 0-3 chosen; row 3 unchecked by the person. The calendar asked before
    // held rows 1 and 2 (unchecked then); this one holds 2 and 3.
    const next = rechosenForHeld(new Set([0]), new Set([1, 2]), new Set([2, 3]));
    expect([...next].sort()).toEqual([0, 1]);
    // Nothing held: every choice stays.
    expect([...rechosenForHeld(new Set([0, 2]), new Set(), new Set())].sort()).toEqual([0, 2]);
  });
});

describe('heldStatusSentence', () => {
  const events: ImportedEvent[] = [
    { ...invitation, title: 'Jour fixe', uid: 'jf@example.org' },
    { ...invitation, title: 'Budget', uid: 'b@example.org' },
  ];
  it('says what is asked, what failed, and names what the calendar holds', () => {
    expect(heldStatusSentence(events, { status: 'checking' }, 'Arbeit', t)).toBe(
      'Aperio prüft, was davon schon in „Arbeit“ steht …',
    );
    expect(heldStatusSentence(events, { status: 'failed' }, 'Arbeit', t)).toBe(
      'Aperio konnte nicht prüfen, was davon schon in „Arbeit“ steht.',
    );
    expect(heldStatusSentence(events, { status: 'known', held: new Set() }, 'Arbeit', t)).toBe(
      'Nichts davon steht schon in „Arbeit“.',
    );
    expect(
      heldStatusSentence(events, { status: 'known', held: new Set([0, 1]) }, 'Arbeit', t),
    ).toBe(
      'Schon in „Arbeit“: „Jour fixe“ und „Budget“. Abgewählt; wer einen anwählt, importiert ' +
        'ihn als Kopie.',
    );
    expect(heldSentence(events, new Set([1]), 'Arbeit', t)).toMatch(/^Schon in „Arbeit“: „Budget“\./);
  });
  it('says nothing for a file that names no UID', () => {
    const noUid = events.map(({ uid: _uid, ...event }) => event);
    expect(heldStatusSentence(noUid, { status: 'checking' }, 'Arbeit', t)).toBeNull();
  });
});

describe('invitationListSentence', () => {
  it('says how the calendar takes the invitations, once whose they are is known', () => {
    const newOne = { kind: 'new' } as const;
    expect(invitationListSentence([newOne], 'Arbeit', t)).toBeNull();
    expect(
      invitationListSentence([newOne, { kind: 'checking' }], 'Arbeit', t),
    ).toBeNull();
    expect(
      invitationListSentence([newOne, { kind: 'invitation', tentative: false }], 'Arbeit', t),
    ).toBe(t('fileImport.invitation.listKept', { calendar: 'Arbeit' }));
    expect(
      invitationListSentence([{ kind: 'invitation', tentative: true }], 'Arbeit', t),
    ).toBe(t('fileImport.invitation.listKeptTentative', { calendar: 'Arbeit' }));
    expect(invitationListSentence([{ kind: 'set-aside' }], 'Arbeit', t)).toBe(
      t('fileImport.invitation.listSetAside', { calendar: 'Arbeit' }),
    );
  });
});

describe('importWhen and importRowLabel', () => {
  const format = {
    date: (at: Date) => at.toISOString().slice(0, 10),
    time: (at: Date) => at.toISOString().slice(11, 16),
  };
  it('says the day, the times or all day, and a series', () => {
    expect(importWhen(invitation, format, t)).toBe('2026-11-18, 13:00–14:00');
    expect(importWhen({ ...invitation, all_day: true }, format, t)).toBe('2026-11-18, ganztägig');
    expect(
      importWhen(
        { ...invitation, recurrence: { rrule: 'FREQ=WEEKLY', exceptions: [] } },
        format,
        t,
      ),
    ).toBe('2026-11-18, 13:00–14:00, Serie');
  });
  it('names a held row, and the copy it becomes while checked', () => {
    expect(importRowLabel(invitation, 'morgen', false, true, 'Arbeit', t)).toBe(
      'Projektbesprechung, morgen',
    );
    expect(importRowLabel(invitation, 'morgen', true, false, 'Arbeit', t)).toBe(
      'Projektbesprechung, morgen, steht schon in „Arbeit“',
    );
    expect(importRowLabel(invitation, 'morgen', true, true, 'Arbeit', t)).toBe(
      'Projektbesprechung, morgen, steht schon in „Arbeit“, wird als Kopie importiert',
    );
  });
});

describe('importedCreate', () => {
  const entry: ImportedEvent = { ...invitation, uid: 'pb@example.org' };
  it('writes an invitation as theirs, telling nobody, under the file’s UID', () => {
    const create = importedCreate(entry, { kind: 'invitation', tentative: false }, 'cal-1', false);
    expect(create).toMatchObject({
      calendar_id: 'cal-1',
      title: 'Projektbesprechung',
      organizer: 'erika@example.org',
      organized_elsewhere: true,
      keeps_invitation: true,
      attendees: ['Max <max@example.org>'],
      send_invitations: false,
      uid_to_keep: 'pb@example.org',
      use_calendar_defaults: true,
    });
  });
  it('makes a held one a copy, and keeps the file’s alarms as the choice', () => {
    const withAlarm: ImportedEvent = {
      ...entry,
      reminders: [{ kind: { type: 'relative', minutes_before: 15 }, sound: null }],
    };
    const create = importedCreate(withAlarm, { kind: 'set-aside' }, 'cal-1', true);
    expect(create.uid_to_keep).toBeNull();
    expect(create.use_calendar_defaults).toBe(false);
    expect(create.attendees).toEqual([]);
    expect(create.keeps_invitation).toBe(false);
  });
});
