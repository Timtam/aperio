import { describe, expect, it } from 'vitest';
import {
  alreadyHeld,
  importFailureSentence,
  importInto,
  importIntoSentence,
  importOwner,
  importPeople,
  importSpan,
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
