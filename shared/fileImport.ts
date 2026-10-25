// File import (DESIGN-file-import.md): the rules both shells share once a
// plugin has read a file — which appointment the editor gets, and how what
// was left out is said.

import { codedError, errorMessageText } from './eventWriteError';
import { errorMessage } from './errors';
import { joinNames } from './listWords';
import type { ImportedEvent } from './generated/ImportedEvent';
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

/**
 * The appointment the editor is filled with, and whether an invitation was
 * set aside.
 *
 * Until each provider has its own quiet way to keep an invitation someone
 * else organizes (I6), an appointment the file names an organizer for comes
 * without its organizer and invitees (Toni, 2026-10-26): written as they are,
 * Exchange would make it the account's own meeting, and iCloud would mail
 * every invitee. One without an organizer keeps its invitees, as a new
 * appointment does.
 */
export function importedForEditor(event: ImportedEvent): {
  event: ImportedEvent;
  invitationSetAside: boolean;
} {
  if (!event.organizer) return { event, invitationSetAside: false };
  const { organizer: _organizer, ...rest } = event;
  return { event: { ...rest, attendees: [] }, invitationSetAside: true };
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
