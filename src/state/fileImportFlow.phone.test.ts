import { describe, expect, it, vi } from 'vitest';

import type { FileImport, ImportedEvent } from '@aperio/shared';

import i18n from '../i18n';

/**
 * What a calendar file opens on the phone (DESIGN-file-import.md, I5), run
 * here because the phone has no test runner: one appointment in the editor,
 * several in the list, a sentence for a file that failed or held nothing; the
 * calendar it goes to first; and how the editor tells the list which rows it
 * created.
 *
 * Loaded through paths the type checker does not follow: the desktop's tsc
 * must not pull the phone's module graph in (CI has no mobile packages).
 */
const FLOW_MODULE = '../../mobile/src/state/fileImportFlow';
const PROGRESS_MODULE = '../../mobile/src/state/importProgress';
const ROUTE_ID_MODULE = '../../mobile/src/navigation/importRouteId';

type Translate = (key: string, values?: Record<string, unknown>) => string;

interface Presenter {
  t: Translate;
  read: (path: string) => Promise<FileImport>;
  target: () => Promise<string | null>;
  openEditor: (calendarId: string, event: ImportedEvent, note: string | null) => void;
  openList: (fileName: string, read: FileImport) => void;
  say: (sentence: string) => void;
  log: (message: string) => void;
}

interface Flow {
  readAndPresent: (fileName: string, path: string, p: Presenter) => Promise<void>;
  pathOfPicked: (uri: string) => string;
  incomingFileOfUrl: (url: string) => { path: string; name: string } | null;
  importTarget: (
    calendars: readonly { id: string; read_only: boolean }[],
    lastUsed: string | null,
    hidden: ReadonlySet<string>,
    includeHidden: boolean,
  ) => string | null;
}

interface Progress {
  markImportedFromEditor: (token: string, index: number) => void;
  onImportedFromEditor: (token: string, listener: (index: number) => void) => () => void;
  newImportToken: () => string;
}

const flow = async () => (await import(/* @vite-ignore */ FLOW_MODULE)) as Flow;
const progress = async () => (await import(/* @vite-ignore */ PROGRESS_MODULE)) as Progress;

const t: Translate = (key, values) => i18n.t(key, values);

const event = (title: string): ImportedEvent => ({
  title,
  start: '2026-11-18T13:00:00Z',
  end: '2026-11-18T14:00:00Z',
  all_day: false,
  reminders: [],
  attendees: [],
});

function presenter(read: Presenter['read'], target: string | null = 'cal-1') {
  const opened: { editor?: unknown[]; list?: unknown[] } = {};
  const said: string[] = [];
  const logged: string[] = [];
  const p: Presenter = {
    t,
    read,
    target: vi.fn(async () => target),
    openEditor: (...args) => {
      opened.editor = args;
    },
    openList: (...args) => {
      opened.list = args;
    },
    say: (sentence) => said.push(sentence),
    log: (message) => logged.push(message),
  };
  return { p, opened, said, logged };
}

describe('readAndPresent', () => {
  it('opens one appointment in the editor, in the calendar it goes to first', async () => {
    const { readAndPresent } = await flow();
    const jour = event('Jour fixe');
    const { p, opened, said } = presenter(async () => ({
      events: [jour],
      skipped: [{ kind: 'task', title: 'Geschenke besorgen' }],
    }));
    await readAndPresent('jour.ics', '/cache/jour.ics', p);
    expect(opened.editor).toEqual([
      'cal-1',
      jour,
      'Nicht übernommen: die Aufgabe „Geschenke besorgen“.',
    ]);
    expect(opened.list).toBeUndefined();
    expect(said).toEqual([]);
  });

  it('opens several in the list, without asking for a calendar', async () => {
    const { readAndPresent } = await flow();
    const read: FileImport = { events: [event('A'), event('B')], skipped: [] };
    const { p, opened } = presenter(async () => read);
    await readAndPresent('export.ics', '/cache/export.ics', p);
    expect(opened.list).toEqual(['export.ics', read]);
    expect(p.target).not.toHaveBeenCalled();
  });

  it('says what a file without an appointment held, and why one failed', async () => {
    const { readAndPresent } = await flow();
    const empty = presenter(async () => ({ events: [], skipped: [{ kind: 'journal' }] }));
    await readAndPresent('notes.ics', '/cache/notes.ics', empty.p);
    expect(empty.said).toEqual([
      'In „notes.ics“ steht kein Termin, der sich importieren lässt. ' +
        'Nicht übernommen: eine Notiz ohne Titel.',
    ]);

    const failed = presenter(async () => {
      throw { code: 'unsupported', message: 'no plugin reads .pdf' };
    });
    await readAndPresent('scan.pdf', '/cache/scan.pdf', failed.p);
    expect(failed.said).toEqual([
      '„scan.pdf“ ließ sich nicht importieren: Aperio kann Dateien dieser Art nicht importieren.',
    ]);
    // The plugin's own words go to the log, not on screen.
    expect(failed.logged).toEqual(['reading a file for an import failed: no plugin reads .pdf']);
  });

  it('says so when no calendar takes the one appointment', async () => {
    const { readAndPresent } = await flow();
    const { p, opened, said } = presenter(async () => ({ events: [event('A')], skipped: [] }), null);
    await readAndPresent('a.ics', '/cache/a.ics', p);
    expect(opened.editor).toBeUndefined();
    expect(said).toEqual(['Es gibt keinen Kalender, in den Aperio Termine schreiben kann.']);
  });
});

describe('pathOfPicked', () => {
  it('turns a picked file URI into the path the plugin reads', async () => {
    const { pathOfPicked } = await flow();
    expect(pathOfPicked('file:///data/cache/Mein%20Export.ics')).toBe('/data/cache/Mein Export.ics');
    expect(pathOfPicked('/already/a/path.ics')).toBe('/already/a/path.ics');
  });
});

describe('incomingFileOfUrl', () => {
  it('takes a file an "Open in" handed over, and nothing else', async () => {
    const { incomingFileOfUrl } = await flow();
    expect(
      incomingFileOfUrl('file:///private/var/mobile/Containers/Data/Inbox/Jour%20fixe.ics'),
    ).toEqual({
      path: '/private/var/mobile/Containers/Data/Inbox/Jour fixe.ics',
      name: 'Jour fixe.ics',
    });
    // The OAuth callback comes the same way, and is no file.
    expect(incomingFileOfUrl('aperio://oauth-callback?code=1')).toBeNull();
    expect(incomingFileOfUrl('file:///')).toBeNull();
  });
});

describe('importTarget', () => {
  const calendars = [
    { id: 'feed', read_only: true },
    { id: 'home', read_only: false },
    { id: 'work', read_only: false },
    { id: 'hidden', read_only: false },
  ];
  it('takes the calendar last created on while it is offered, else the first offered', async () => {
    const { importTarget } = await flow();
    const hidden = new Set(['hidden']);
    expect(importTarget(calendars, 'work', hidden, false)).toBe('work');
    expect(importTarget(calendars, 'feed', hidden, false)).toBe('home');
    expect(importTarget(calendars, 'hidden', hidden, false)).toBe('home');
    expect(importTarget(calendars, 'hidden', hidden, true)).toBe('hidden');
    expect(importTarget(calendars, null, hidden, false)).toBe('home');
    expect(importTarget([{ id: 'feed', read_only: true }], null, new Set(), false)).toBeNull();
  });
});

describe('importRouteId', () => {
  it('gives an editor on a file a route of its own, and every other the default', async () => {
    const { importRouteId } = (await import(/* @vite-ignore */ ROUTE_ID_MODULE)) as {
      importRouteId: (route: { params: Record<string, unknown> }) => string | undefined;
    };
    expect(importRouteId({ params: { eventId: null, calendarId: 'c' } })).toBeUndefined();
    expect(
      importRouteId({ params: { eventId: null, calendarId: 'c', importToken: 'import-1' } }),
    ).toBe('import-1:');
    expect(
      importRouteId({
        params: { eventId: null, calendarId: 'c', importToken: 'import-1', importIndex: 2 },
      }),
    ).toBe('import-1:2');
  });
});

describe('importProgress', () => {
  it('tells the list behind a token which rows the editor created, until it stops listening', async () => {
    const { markImportedFromEditor, onImportedFromEditor, newImportToken } = await progress();
    const token = newImportToken();
    const other = newImportToken();
    expect(other).not.toBe(token);
    const heard: number[] = [];
    const stop = onImportedFromEditor(token, (index) => heard.push(index));
    markImportedFromEditor(token, 2);
    markImportedFromEditor(other, 5);
    stop();
    markImportedFromEditor(token, 3);
    expect(heard).toEqual([2]);
  });
});
