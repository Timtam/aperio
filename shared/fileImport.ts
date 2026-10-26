// File import (DESIGN-file-import.md): the rules both shells share once a
// plugin has read a file — which appointment the editor gets, and how what
// was left out is said.

import { codedError, errorMessageText } from './eventWriteError';
import { errorMessage } from './errors';
import { joinNames } from './listWords';
import type { ImportedEvent } from './generated/ImportedEvent';
import type { InvitationImport } from './generated/InvitationImport';
import type { SkippedPart } from './generated/SkippedPart';

type Translate = (key: string, values?: Record<string, unknown>) => string;

/** The token a too large file's error starts with (`host_core::file_import::TOO_LARGE`). */
const TOO_LARGE = 'import-too-large';

/**
 * Why a file could not be imported, in the reader's words. The host says
 * which kind of failure it was by the error's code; the plugin's own detail
 * (a parser's English) goes to the log, not on screen.
 */
export function importFailureSentence(fileName: string, err: unknown, t: Translate): string {
  const code = codedError(err)?.code;
  const message = errorMessageText(err);
  const reason =
    code === 'unsupported'
      ? t('fileImport.reason.unsupported')
      : code === 'invalid_input' && message.startsWith(TOO_LARGE)
        ? t('fileImport.reason.tooLarge')
        : code === 'invalid_input'
          ? t('fileImport.reason.unreadable')
          : code === 'internal'
            ? t('fileImport.reason.notOpened')
            : errorMessage(err);
  return t('fileImport.failed', { file: fileName, reason });
}

/** Who organizes an appointment read from a file, as far as the calendar it
 *  goes to can tell: nobody named, the account itself, someone else — or not
 *  known yet, while the calendar is asked for the account's address. */
export type ImportOwner = 'none' | 'me' | 'someone-else' | 'checking';

/**
 * Who organizes `event`, given the account's address in the target calendar
 * (`undefined` while it is asked, `null` when the calendar has none to give).
 * Addresses compare as `cal_core::attendee::normalize_address` does: no
 * `mailto:`, no case. The adapter asks again with every address the account
 * has when it creates the appointment, so an alias the surface cannot know
 * still reads as the account's own there.
 */
export function importOwner(event: ImportedEvent, own: string | null | undefined): ImportOwner {
  if (!event.organizer?.trim()) return 'none';
  if (own === undefined) return 'checking';
  const normalized = (address: string) => address.trim().replace(/^mailto:/i, '').trim().toLowerCase();
  return own && normalized(own) === normalized(event.organizer) ? 'me' : 'someone-else';
}

/**
 * How an appointment read from a file goes into one calendar
 * (DESIGN-file-import.md, I6):
 *
 * - `new`: nobody organizes it, or the account does — a new appointment, its
 *   invitees kept, editable as any, notified by the usual rules;
 * - `invitation`: someone else organizes it and the calendar keeps
 *   invitations — created as theirs, as the file has it, nobody told
 *   (`tentative`: the account's answer reads "tentative" afterwards);
 * - `set-aside`: someone else organizes it and the calendar has no quiet way
 *   — the account's own appointment, without the organizer and the invitees;
 * - `checking`: not known yet whose it is.
 */
export type ImportInto =
  | { kind: 'new' }
  | { kind: 'invitation'; tentative: boolean }
  | { kind: 'set-aside' }
  | { kind: 'checking' };

export function importInto(
  owner: ImportOwner,
  calendar: { invitation_import?: InvitationImport } | undefined,
): ImportInto {
  if (owner === 'checking') return { kind: 'checking' };
  if (owner !== 'someone-else') return { kind: 'new' };
  const keeps = calendar?.invitation_import ?? 'not-kept';
  return keeps === 'not-kept'
    ? { kind: 'set-aside' }
    : { kind: 'invitation', tentative: keeps === 'kept-tentative' };
}

/**
 * The create an appointment from a file makes in a calendar it goes into as
 * `into` says: the fields the provider keeps for an invitation, and the
 * people. The surfaces add their calendar, the UID and the content.
 */
export function importPeople(
  event: ImportedEvent,
  into: ImportInto,
): {
  attendees: string[];
  organizer: string | null;
  organized_elsewhere: boolean;
  keeps_invitation: boolean;
} {
  switch (into.kind) {
    case 'invitation':
      return {
        attendees: event.attendees,
        organizer: event.organizer ?? null,
        organized_elsewhere: true,
        keeps_invitation: true,
      };
    case 'set-aside':
      return { attendees: [], organizer: null, organized_elsewhere: false, keeps_invitation: false };
    default:
      // The account's own meeting names its organizer so the host keeps the
      // account out of its invitees (decision 72a); nobody else is told
      // anything the usual rules do not tell them.
      return {
        attendees: event.attendees,
        organizer: event.organizer ?? null,
        organized_elsewhere: false,
        keeps_invitation: false,
      };
  }
}

/**
 * The people an editor's create carries (I6): someone else's meeting the
 * calendar keeps goes in as theirs, as the file has it (`importPeople`);
 * anything else carries the form's invitees — and the account's own meeting
 * from a file its organizer, so the host keeps the account out of its
 * invitees (decision 72a). `imported` is `null` for an appointment not from a
 * file.
 */
export function importEditorPeople(
  imported: ImportedEvent | null,
  into: ImportInto | null,
  attendees: string[],
): {
  attendees: string[];
  organizer: string | null;
  organized_elsewhere: boolean;
  keeps_invitation: boolean;
} {
  if (imported && into?.kind === 'invitation') return importPeople(imported, into);
  return {
    attendees,
    organizer: into?.kind === 'new' ? (imported?.organizer ?? null) : null,
    organized_elsewhere: false,
    keeps_invitation: false,
  };
}

/** What the editor says about an appointment going into `calendar` as
 *  `into` says, or `null` when there is nothing to say. */
export function importIntoSentence(
  event: ImportedEvent,
  into: ImportInto,
  calendar: string,
  t: Translate,
): string | null {
  const organizer = event.organizer?.trim() ?? '';
  switch (into.kind) {
    case 'invitation':
      return t(into.tentative ? 'fileImport.invitation.keptTentative' : 'fileImport.invitation.kept', {
        organizer,
      });
    case 'set-aside':
      return t('fileImport.invitation.setAside', { organizer, calendar });
    default:
      return null;
  }
}

/** One part a file held that is not imported, in words. */
function skippedPartText(part: SkippedPart, t: Translate): string {
  const title = part.title?.trim();
  return title
    ? t(`fileImport.skipped.${part.kind}.titled`, { title })
    : t(`fileImport.skipped.${part.kind}.untitled`);
}

/**
 * What the file held that is not imported, as one sentence — each part by
 * its kind and title, never a count (reference: name, don't count) — or
 * `null` when everything came along.
 */
export function skippedSentence(skipped: readonly SkippedPart[], t: Translate): string | null {
  if (skipped.length === 0) return null;
  const parts = skipped.map((part) => skippedPartText(part, t));
  return t('fileImport.skipped.sentence', { parts: joinNames(parts, t) });
}

/** A minute, in milliseconds. */
const MINUTE = 60_000;

/**
 * The stretch to ask a calendar about before an import (I4): from the
 * earliest start to the latest end of the file's appointments, a minute wider
 * either side so one that takes no time is found too; `null` for none. A
 * calendar lists a series it holds by an occurrence in the stretch, and the
 * same series starts where the file's does.
 */
export function importSpan(events: readonly ImportedEvent[]): { start: string; end: string } | null {
  let start = Infinity;
  let end = -Infinity;
  for (const event of events) {
    start = Math.min(start, Date.parse(event.start));
    end = Math.max(end, Date.parse(event.end));
  }
  if (!Number.isFinite(start) || !Number.isFinite(end)) return null;
  return {
    start: new Date(start - MINUTE).toISOString(),
    end: new Date(end + MINUTE).toISOString(),
  };
}

/**
 * Which of a file's appointments a calendar already holds, by their place in
 * the file: those whose UID one of the calendar's rows carries (I4). One
 * without a UID — a changed occurrence the file gave as an appointment of its
 * own, a file that names none — never is: nothing says it is the same one.
 */
export function alreadyHeld(
  events: readonly ImportedEvent[],
  rows: readonly { ical_uid?: string | null }[],
): Set<number> {
  const held = new Set(
    rows.map((row) => row.ical_uid?.trim()).filter((uid): uid is string => !!uid),
  );
  const out = new Set<number>();
  events.forEach((event, index) => {
    const uid = event.uid?.trim();
    if (uid && held.has(uid)) out.add(index);
  });
  return out;
}

/** What a calendar already holds of a file's appointments (I4): still being
 *  asked, could not be asked, or which ones, by their place in the file. */
export type HeldCheck =
  | { status: 'checking' }
  | { status: 'failed' }
  | { status: 'known'; held: ReadonlySet<number> };

/**
 * The rows an import list keeps chosen once a calendar answered what it holds
 * (I4): those it holds unchecked, and those unchecked for the calendar asked
 * before (`before`) checked again where this one does not hold them. Every
 * other choice the person made stays.
 */
export function rechosenForHeld(
  chosen: ReadonlySet<number>,
  before: ReadonlySet<number>,
  held: ReadonlySet<number>,
): Set<number> {
  const next = new Set(chosen);
  for (const index of before) if (!held.has(index)) next.add(index);
  for (const index of held) next.delete(index);
  return next;
}

/** The appointments at `rows` of a file, named as the ones `calendar` holds
 *  already: each by its title, never a count. */
export function heldSentence(
  events: readonly ImportedEvent[],
  rows: ReadonlySet<number>,
  calendar: string,
  t: Translate,
): string {
  return t('fileImport.held.some', {
    calendar,
    titles: joinNames(
      events
        .filter((_, index) => rows.has(index))
        .map((event) => t('fileImport.list.quoted', { title: event.title })),
      t,
    ),
  });
}

/** What an import list says about what `calendar` holds of the file, or
 *  `null` when the file names no UID and there was nothing to ask. */
export function heldStatusSentence(
  events: readonly ImportedEvent[],
  check: HeldCheck,
  calendar: string,
  t: Translate,
): string | null {
  if (!events.some((event) => event.uid?.trim())) return null;
  switch (check.status) {
    case 'checking':
      return t('fileImport.held.checking', { calendar });
    case 'failed':
      return t('fileImport.held.checkFailed', { calendar });
    default:
      return check.held.size === 0
        ? t('fileImport.held.none', { calendar })
        : heldSentence(events, check.held, calendar, t);
  }
}

/**
 * What an import list says about the meetings someone else organizes among a
 * file's appointments, for the calendar chosen (I6): kept, kept with the
 * answer tentative, or set aside — or `null` while whose one is is still
 * asked, or when none is someone else's. The calendar decides alike for all.
 */
export function invitationListSentence(
  intos: readonly ImportInto[],
  calendar: string,
  t: Translate,
): string | null {
  const invitations = intos.filter((into) => into.kind !== 'new');
  if (invitations.length === 0 || invitations.some((into) => into.kind === 'checking')) {
    return null;
  }
  const first = invitations[0];
  if (first.kind === 'set-aside') return t('fileImport.invitation.listSetAside', { calendar });
  return t(
    first.kind === 'invitation' && first.tentative
      ? 'fileImport.invitation.listKeptTentative'
      : 'fileImport.invitation.listKept',
    { calendar },
  );
}

/** When an appointment from a file takes place, as an import list says it:
 *  its day, its times unless it lasts all day, and that it repeats. `format`
 *  writes a day and a time of day the surface's way. */
export function importWhen(
  event: ImportedEvent,
  format: { date: (at: Date) => string; time: (at: Date) => string },
  t: Translate,
): string {
  const start = new Date(event.start);
  const text = event.all_day
    ? t('fileImport.list.allDay', { date: format.date(start) })
    : t('fileImport.list.timed', {
        date: format.date(start),
        time: `${format.time(start)}–${format.time(new Date(event.end))}`,
      });
  return event.recurrence ? `${text}, ${t('fileImport.list.series')}` : text;
}

/** One row of an import list: the title, when, and — where `calendar` holds
 *  it already — that it is there, or goes in as a copy while checked. */
export function importRowLabel(
  event: ImportedEvent,
  when: string,
  held: boolean,
  chosen: boolean,
  calendar: string,
  t: Translate,
): string {
  return [
    event.title,
    when,
    held ? t(chosen ? 'fileImport.held.rowCopy' : 'fileImport.held.row', { calendar }) : null,
  ]
    .filter(Boolean)
    .join(', ');
}

/**
 * The create an import list writes for one appointment from a file into
 * `calendarId`, as `into` says (I6), nobody told: its alarms, or the
 * calendar's defaults where the file has none, as for an appointment nobody
 * chose reminders for; and the file's UID, so importing it again finds it
 * (I4) — unless the calendar holds it already (`held`): then it is a copy,
 * under a UID of its own. The surfaces send it keeping the file's zone.
 */
export function importedCreate(
  entry: ImportedEvent,
  into: ImportInto,
  calendarId: string,
  held: boolean,
) {
  return {
    calendar_id: calendarId,
    title: entry.title,
    description: entry.description ?? null,
    location: entry.location ?? null,
    start: entry.start,
    end: entry.end,
    all_day: entry.all_day,
    recurrence: entry.recurrence ?? null,
    color_label: null,
    reminders: entry.reminders,
    sound: null,
    ...importPeople(entry, into),
    send_invitations: false,
    use_calendar_defaults: entry.reminders.length === 0,
    uid_to_keep: held ? null : (entry.uid ?? null),
  };
}
