import { useCallback, useEffect, useId, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  createsInSteps,
  heldSentence,
  heldStatusSentence,
  importedCreate,
  importFailureSentence,
  importInto,
  importOwner,
  importRowLabel,
  importWhen,
  invitationListSentence,
  partlyImportedSentence,
  rechosenForHeld,
  selectableEventCalendars,
  skippedSentence,
  writeNeverLanded,
} from '@aperio/shared';

import { FocusableNote } from '../a11y/FocusableNote';
import { useAnnouncer } from '../a11y/announcerContext';
import { createEvent as apiCreateEvent } from '../api/client';
import type { FileImport, ImportedEvent } from '../api/types';
import { useDateFormat } from '../intl/dateFormat';
import { useCalendarStore } from '../state/calendarStoreContext';
import type { FileImportOutcome } from '../state/DialogState';
import { useDialogState } from '../state/dialogStateContext';
import { useCalendarOwnAddress } from '../state/useCalendarOwnAddress';
import { useHeldOnImport } from '../state/useHeldOnImport';
import { useViewState } from '../state/viewStateContext';
import { readLastUsedCalendar, writeLastUsedCalendar } from './lastUsedCalendar';
import { Modal } from './Modal';
import { useStepWriteWait } from './useStepWriteWait';

interface ImportDialogProps {
  isOpen: boolean;
  onClose: () => void;
  /** The file's name, as the sentences name it. */
  fileName: string;
  outcome: FileImportOutcome;
}

/**
 * What a calendar file came to (DESIGN-file-import.md): the appointments it
 * holds when there is more than one — each to open in the editor, or all
 * chosen ones written to one calendar —, or why nothing could be imported.
 * One appointment never reaches this dialog: the editor opens on it.
 */
export function ImportDialog({ isOpen, onClose, fileName, outcome }: ImportDialogProps) {
  const { t } = useTranslation();
  const title = t('fileImport.dialogTitle');
  if ('failed' in outcome) {
    return (
      <ImportNotice
        isOpen={isOpen}
        onClose={onClose}
        title={title}
        sentence={importFailureSentence(fileName, outcome.failed, t)}
      />
    );
  }
  if (outcome.read.events.length === 0) {
    const sentence = [
      t('fileImport.nothing', { file: fileName }),
      skippedSentence(outcome.read.skipped, t),
    ]
      .filter(Boolean)
      .join(' ');
    return <ImportNotice isOpen={isOpen} onClose={onClose} title={title} sentence={sentence} />;
  }
  return (
    <ImportList
      isOpen={isOpen}
      onClose={onClose}
      title={title}
      fileName={fileName}
      read={outcome.read}
    />
  );
}

/** A sentence and Close: a file that failed, or held nothing to import. */
function ImportNotice({
  isOpen,
  onClose,
  title,
  sentence,
}: {
  isOpen: boolean;
  onClose: () => void;
  title: string;
  sentence: string;
}) {
  const { t } = useTranslation();
  return (
    <Modal isOpen={isOpen} onClose={onClose} title={title} className="modal--form">
      <div className="form">
        <FocusableNote className="form__hint form__hint--warning">{sentence}</FocusableNote>
        <div className="form__actions">
          <button type="button" className="form__action form__action--primary" onClick={onClose}>
            {t('dialogs.close')}
          </button>
        </div>
      </div>
    </Modal>
  );
}

/**
 * What a list has done with a file so far: the rows chosen and imported, the
 * calendar, and the answer about what that calendar holds that was taken. It
 * outlives the list: "Edit …" puts the editor on top of the dialog stack,
 * which unmounts the list, and the editor tells it here which row it created.
 * Keyed by the file's read, which the dialog stack keeps.
 */
interface ListProgress {
  chosen: Set<number>;
  imported: Set<number>;
  /** Of `imported`, the rows that stand only in part. */
  inPart: Set<number>;
  calendarId: string | null;
  /** The answer taken last, so it is not taken again on the way back. */
  answered: { calendarId: string; held: ReadonlySet<number> } | null;
}

const listProgress = new WeakMap<FileImport, ListProgress>();

function progressOf(read: FileImport): ListProgress {
  let progress = listProgress.get(read);
  if (!progress) {
    progress = {
      chosen: new Set(read.events.map((_, i) => i)),
      imported: new Set(),
      inPart: new Set(),
      calendarId: null,
      answered: null,
    };
    listProgress.set(read, progress);
  }
  return progress;
}

const sameRows = (a: ReadonlySet<number>, b: ReadonlySet<number>) =>
  a.size === b.size && [...a].every((index) => b.has(index));

function ImportList({
  isOpen,
  onClose,
  title,
  fileName,
  read,
}: {
  isOpen: boolean;
  onClose: () => void;
  title: string;
  fileName: string;
  read: FileImport;
}) {
  const { t } = useTranslation();
  const fmt = useDateFormat();
  const announce = useAnnouncer();
  const { openEventDialog, invalidateData } = useDialogState();
  const { calendars, selectedCalendarIds } = useCalendarStore();
  const { showHiddenCalendarTargets } = useViewState();
  const calendarFieldId = useId();
  const noticeRef = useRef<HTMLParagraphElement>(null);
  const progress = progressOf(read);

  const targets = useMemo(
    () =>
      selectableEventCalendars(calendars, {
        selectedIds: selectedCalendarIds,
        includeHidden: showHiddenCalendarTargets,
      }),
    [calendars, selectedCalendarIds, showHiddenCalendarTargets],
  );
  const [calendarId, setCalendarIdState] = useState<string>(() => {
    const kept = progress.calendarId ?? readLastUsedCalendar();
    return targets.some((c) => c.id === kept) ? (kept as string) : (targets[0]?.id ?? '');
  });
  const setCalendarId = useCallback(
    (id: string) => {
      progress.calendarId = id;
      setCalendarIdState(id);
    },
    [progress],
  );
  // The catalog may arrive after the dialog: keep a target that exists.
  useEffect(() => {
    if (!targets.some((c) => c.id === calendarId) && targets[0]) {
      setCalendarId(targets[0].id);
    }
  }, [targets, calendarId, setCalendarId]);
  const calendarName = targets.find((c) => c.id === calendarId)?.name ?? '';

  const events = read.events;
  const [chosen, setChosenState] = useState<ReadonlySet<number>>(() => new Set(progress.chosen));
  const setChosen = useCallback(
    (next: (prev: ReadonlySet<number>) => ReadonlySet<number>) => {
      setChosenState((prev) => {
        const value = next(prev);
        progress.chosen = new Set(value);
        return value;
      });
    },
    [progress],
  );
  const [imported, setImported] = useState<ReadonlySet<number>>(
    () => new Set(progress.imported),
  );
  const [inPart, setInPart] = useState<ReadonlySet<number>>(() => new Set(progress.inPart));
  const { waitDialog, wait, hold } = useStepWriteWait();
  const [busy, setBusy] = useState(false);
  // Each notice is a value of its own: the same sentence for a second press
  // is focused and said again.
  const [notice, setNoticeState] = useState<{ text: string; seq: number } | null>(null);
  const setNotice = useCallback((text: string | null) => {
    setNoticeState((prev) => (text == null ? null : { text, seq: (prev?.seq ?? 0) + 1 }));
  }, []);
  useEffect(() => {
    if (notice) noticeRef.current?.focus();
  }, [notice]);

  // What the chosen calendar holds already (I4): unchecked when the answer
  // comes, and imported as a copy, under a UID of its own, when checked again.
  const calendar = targets.find((c) => c.id === calendarId);
  const held = useHeldOnImport(calendarId, events, calendar?.keeps_uid);
  const isHeld = (index: number) => held.status === 'known' && held.held.has(index);

  // Whose each one is, as the chosen calendar tells: the account's own meeting
  // is a new appointment, someone else's an invitation the calendar keeps or
  // cannot keep quietly (I6).
  const own = useCalendarOwnAddress(
    calendarId,
    events.some((event) => event.organizer?.trim()),
  );
  const intoOf = (event: ImportedEvent) => importInto(importOwner(event, own), calendar);
  const invitationSentence = calendarId
    ? invitationListSentence(events.map(intoOf), calendarName, t)
    : null;

  // Each held answer is taken once: its rows unchecked, and the rows unchecked
  // for the calendar asked before checked again where this one does not hold
  // them. An answer taken before the list was unmounted is not taken again.
  // What changed is said in one announcement: a second one in the same frame
  // would replace the first.
  // A calendar that cannot tell answers with one and the same value for
  // every calendar: an answer is told apart by its calendar too.
  const lastAnswer = useRef<{ calendarId: string; held: typeof held } | null>(null);
  const uncheckedRows = useRef<ReadonlySet<number>>(progress.answered?.held ?? new Set());
  const saidInvitation = useRef<string | null>(null);
  useEffect(() => {
    const said: string[] = [];
    const fresh =
      lastAnswer.current?.held !== held || lastAnswer.current?.calendarId !== calendarId;
    if (held.status !== 'checking' && fresh) {
      const first = lastAnswer.current === null;
      lastAnswer.current = { calendarId, held };
      if (held.status === 'failed') {
        said.push(t('fileImport.held.checkFailed', { calendar: calendarName }));
      } else if (held.status === 'unknowable') {
        // Nothing found, nothing to uncheck: the rows another calendar held
        // are checked again. Said where a change of calendar brought it; on
        // open the note under the picker says it.
        const before = uncheckedRows.current;
        uncheckedRows.current = new Set();
        setChosen((prev) => rechosenForHeld(prev, before, new Set()));
        if (!first) said.push(t('fileImport.held.unknowable', { calendar: calendarName }));
      } else {
        const before = uncheckedRows.current;
        uncheckedRows.current = held.held;
        const taken =
          progress.answered?.calendarId === calendarId &&
          sameRows(progress.answered.held, held.held);
        progress.answered = { calendarId, held: held.held };
        if (!taken) {
          setChosen((prev) => rechosenForHeld(prev, before, held.held));
          if (held.held.size > 0) said.push(heldSentence(events, held.held, calendarName, t));
        }
      }
    }
    if (invitationSentence && saidInvitation.current !== invitationSentence) {
      said.push(invitationSentence);
    }
    saidInvitation.current = invitationSentence;
    if (said.length > 0) announce(said.join(' '));
  }, [held, invitationSentence, announce, calendarId, calendarName, events, progress, setChosen, t]);

  const heldStatus = calendarId ? heldStatusSentence(events, held, calendarName, t) : null;
  const summary = [
    t('fileImport.list.summary', { file: fileName, count: events.length }),
    skippedSentence(read.skipped, t),
    invitationSentence,
  ]
    .filter(Boolean)
    .join(' ');

  const toImport = events
    .map((event, index) => ({ event, index }))
    .filter(({ index }) => chosen.has(index) && !imported.has(index));

  // Into the progress too: the editor's word arrives while the list is
  // unmounted under it.
  // A row that stands only in part is not offered again either: importing
  // it again would make it twice (decision 277).
  const markImported = useCallback(
    (index: number, onlyInPart = false) => {
      progress.imported.add(index);
      setImported((prev) => new Set(prev).add(index));
      if (onlyInPart) {
        progress.inPart.add(index);
        setInPart((prev) => new Set(prev).add(index));
      }
    },
    [progress],
  );

  const when = (event: ImportedEvent): string =>
    importWhen(
      event,
      { date: (at) => fmt.format(at, 'PPP'), time: (at) => fmt.format(at, 'p') },
      t,
    );

  // As the file has it: the editor decides, for the calendar it ends up in,
  // whether an invitation keeps its people.
  const edit = (event: ImportedEvent, index: number) => {
    openEventDialog(null, {
      calendarId,
      importFrom: event,
      importNote: null,
      onImported: (inPart) => markImported(index, inPart),
    });
  };

  const importChosen = async () => {
    if (busy) return;
    if (!calendarId) {
      setNotice(t('fileImport.noCalendar'));
      return;
    }
    if (toImport.length === 0) {
      setNotice(t('fileImport.list.noneChosen'));
      return;
    }
    // Which are there already decides which keep their UID.
    if (held.status === 'checking') {
      setNotice(t('fileImport.held.stillChecking', { calendar: calendarName }));
      return;
    }
    // Whose they are decides which keep their people.
    if (toImport.some(({ event }) => intoOf(event).kind === 'checking')) {
      setNotice(t('fileImport.invitation.listStillChecking'));
      return;
    }
    setBusy(true);
    setNotice(null);
    const failed: { title: string; err: unknown }[] = [];
    const standing: { title: string; err: unknown }[] = [];
    let done = 0;
    // One after the other: a provider asked many times at once may turn the
    // rest away as busy. The "please wait" dialog opens at the first series
    // that needs it and stays until the last is written.
    const releaseWait = hold();
    for (const { event: entry, index } of toImport) {
      try {
        // A series whose calendar writes its deletions one by one is created
        // in steps: the "please wait" dialog says so (decisions 227, 265).
        await wait(createsInSteps(calendar, entry.recurrence), () =>
          apiCreateEvent(importedCreate(entry, intoOf(entry), calendarId, isHeld(index)), {
            preserveRecurrenceZone: true,
          }),
        );
        done += 1;
        markImported(index);
      } catch (err) {
        // Only one that surely never landed is offered again; any other may
        // stand (decision 277, review round 16 of #128).
        if (writeNeverLanded(err)) {
          failed.push({ title: entry.title, err });
        } else {
          standing.push({ title: entry.title, err });
          markImported(index, true);
        }
      }
    }
    await releaseWait();
    setBusy(false);
    if (done > 0 || standing.length > 0) {
      writeLastUsedCalendar(calendarId);
      invalidateData();
    }
    if (failed.length === 0 && standing.length === 0) {
      announce(t('fileImport.list.done', { count: done, calendar: calendarName }));
      onClose();
      return;
    }
    setNotice(
      partlyImportedSentence(failed, done, toImport.length, calendarName, t, standing),
    );
  };

  return (
    <>
    <Modal
      isOpen={isOpen}
      // Not while it writes: the outcome has to be said where it is seen.
      onClose={() => {
        if (!busy) onClose();
      }}
      title={title}
      className="modal--form"
      dismissOnBackdrop={false}
    >
      <div className="form">
        <FocusableNote className="form__hint">{summary}</FocusableNote>
        {notice && (
          <FocusableNote
            key={notice.seq}
            ref={noticeRef}
            className="form__hint form__hint--warning"
          >
            {notice.text}
          </FocusableNote>
        )}
        <div className="form__field">
          <label className="form__label" htmlFor={calendarFieldId}>
            {t('fileImport.list.calendar')}
          </label>
          <select
            id={calendarFieldId}
            value={calendarId}
            // Held while it writes, without native disabled, which strands
            // focus: controlled, the value snaps back.
            aria-disabled={busy || undefined}
            onChange={(e) => {
              if (!busy) setCalendarId(e.target.value);
            }}
          >
            {targets.map((c) => (
              <option key={c.id} value={c.id}>
                {c.name}
              </option>
            ))}
          </select>
        </div>
        {heldStatus && <FocusableNote className="form__hint">{heldStatus}</FocusableNote>}
        <ul className="import-list" aria-label={t('fileImport.list.listLabel')}>
          {events.map((event, index) => {
            const boxId = `import-${index}`;
            const done = imported.has(index);
            return (
              <li key={boxId} className="import-list__row">
                {done ? (
                  // A stop of its own, so the reader reaches it in the dialog.
                  <FocusableNote className="import-list__done">
                    {`${event.title}, ${when(event)}, ${t(
                      inPart.has(index) ? 'fileImport.list.importedInPart' : 'fileImport.list.imported',
                    )}`}
                  </FocusableNote>
                ) : (
                  <>
                    <input
                      id={boxId}
                      type="checkbox"
                      checked={chosen.has(index)}
                      aria-disabled={busy || undefined}
                      onChange={(e) => {
                        if (busy) return;
                        const checked = e.target.checked;
                        setChosen((prev) => {
                          const next = new Set(prev);
                          if (checked) next.add(index);
                          else next.delete(index);
                          return next;
                        });
                      }}
                    />
                    <label htmlFor={boxId}>
                      {importRowLabel(
                        event,
                        when(event),
                        isHeld(index),
                        chosen.has(index),
                        calendarName,
                        t,
                      )}
                    </label>
                    <button
                      type="button"
                      className="section-field__button"
                      aria-label={t('fileImport.list.editLabel', { title: event.title })}
                      aria-disabled={busy || undefined}
                      onClick={() => {
                        if (!busy) edit(event, index);
                      }}
                    >
                      {t('fileImport.list.edit')}
                    </button>
                  </>
                )}
              </li>
            );
          })}
        </ul>
        <div className="form__actions">
          <button
            type="button"
            className="form__action form__action--secondary"
            aria-disabled={busy || undefined}
            onClick={() => {
              if (!busy) onClose();
            }}
          >
            {t('dialogs.close')}
          </button>
          <button
            type="button"
            className="form__action form__action--primary"
            aria-disabled={busy || toImport.length === 0 || undefined}
            onClick={() => void importChosen()}
          >
            {t('fileImport.list.importChosen', { count: toImport.length })}
          </button>
        </div>
      </div>
    </Modal>
    {waitDialog}
    </>
  );
}
