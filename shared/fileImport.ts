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
