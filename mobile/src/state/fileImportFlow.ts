// What a calendar file opens on the phone (DESIGN-file-import.md, I5): its
// path read through the plugin for its format, then one appointment in the
// editor, several in the import list, or a sentence for a file that failed or
// held nothing to import. No React and no native module here, so the rule is
// tested as it stands (src/state/fileImportFlow.phone.test.ts); the screens
// and, later, a shared or opened file hand in what it needs.

import {
  errorMessageText,
  importFailureSentence,
  selectableEventCalendars,
  skippedSentence,
  type FileImport,
  type ImportedEvent,
} from '@aperio/shared';

type Translate = (key: string, values?: Record<string, unknown>) => string;

export interface ImportPresenter {
  t: Translate;
  /** Read the file at a filesystem path (`readImportFile`). */
  read: (path: string) => Promise<FileImport>;
  /** The calendar a new appointment from the file goes to first, or `null`
   *  when there is none Aperio can write to. */
  target: () => Promise<string | null>;
  openEditor: (calendarId: string, event: ImportedEvent, note: string | null) => void;
  openList: (fileName: string, read: FileImport) => void;
  /** A sentence and nothing else to do: a file that failed, or held nothing. */
  say: (sentence: string) => void;
  /** A line for the app's log (`logLine`): what the person is not shown. */
  log: (message: string) => void;
}

/** Read `path` (the file the person knows as `fileName`) and open what it
 *  holds, as the desktop does. */
export async function readAndPresent(
  fileName: string,
  path: string,
  p: ImportPresenter,
): Promise<void> {
  let read: FileImport;
  try {
    read = await p.read(path);
  } catch (err) {
    // The sentence names the kind of failure; the plugin's own words, a
    // parser's English, belong in the log.
    p.log(`reading a file for an import failed: ${errorMessageText(err)}`);
    p.say(importFailureSentence(fileName, err, p.t));
    return;
  }
  if (read.events.length === 0) {
    p.say(
      [p.t('fileImport.nothing', { file: fileName }), skippedSentence(read.skipped, p.t)]
        .filter(Boolean)
        .join(' '),
    );
    return;
  }
  if (read.events.length > 1) {
    p.openList(fileName, read);
    return;
  }
  const calendarId = await p.target();
  if (calendarId == null) {
    p.say(p.t('fileImport.noCalendar'));
    return;
  }
  p.openEditor(calendarId, read.events[0], skippedSentence(read.skipped, p.t));
}

/** The filesystem path behind a picked file's URI: the plugin reads with
 *  `std::fs`, which knows no `file://` and no percent-escapes. */
export function pathOfPicked(uri: string): string {
  return uri.startsWith('file://') ? decodeURIComponent(uri.replace(/^file:\/\//, '')) : uri;
}

/** The file an "Open in" handed over as a URL (iOS): its path and the name
 *  the person knows it by — `null` for any URL that is no file, such as the
 *  OAuth callback. */
export function incomingFileOfUrl(url: string): { path: string; name: string } | null {
  if (!url.startsWith('file://')) return null;
  const path = pathOfPicked(url);
  const name = path.split('/').filter(Boolean).pop() ?? '';
  return name ? { path, name } : null;
}

/** The calendar an import goes to first: the one last created on, where it
 *  still takes new appointments and is offered, else the first offered — the
 *  writable, shown ones (`selectableEventCalendars`), as the editor's picker
 *  offers them. `null` for none. */
export function importTarget(
  calendars: readonly { id: string; read_only: boolean }[],
  lastUsed: string | null,
  hidden: ReadonlySet<string>,
  includeHidden: boolean,
): string | null {
  const targets = selectableEventCalendars(calendars, {
    selectedIds: new Set(calendars.filter((c) => !hidden.has(c.id)).map((c) => c.id)),
    includeHidden,
  });
  return targets.find((c) => c.id === lastUsed)?.id ?? targets[0]?.id ?? null;
}
