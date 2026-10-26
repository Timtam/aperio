import { useCallback, useEffect, useId, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  heldSentence,
  heldStatusSentence,
  importedCreate,
  importFailureSentence,
  importInto,
  importOwner,
  importRowLabel,
  importWhen,
  invitationListSentence,
  joinNames,
  rechosenForHeld,
  selectableEventCalendars,
  skippedSentence,
  writeErrorMessage,
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

  const targets = useMemo(
    () =>
      selectableEventCalendars(calendars, {
        selectedIds: selectedCalendarIds,
        includeHidden: showHiddenCalendarTargets,
      }),
    [calendars, selectedCalendarIds, showHiddenCalendarTargets],
  );
  const [calendarId, setCalendarId] = useState<string>(() => {
    const last = readLastUsedCalendar();
    return targets.some((c) => c.id === last) ? (last as string) : (targets[0]?.id ?? '');
  });
  // The catalog may arrive after the dialog: keep a target that exists.
  useEffect(() => {
    if (!targets.some((c) => c.id === calendarId) && targets[0]) {
      setCalendarId(targets[0].id);
    }
  }, [targets, calendarId]);
  const calendarName = targets.find((c) => c.id === calendarId)?.name ?? '';

  const events = read.events;
  const [chosen, setChosen] = useState<ReadonlySet<number>>(
    () => new Set(events.map((_, i) => i)),
  );
  const [imported, setImported] = useState<ReadonlySet<number>>(() => new Set());
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  useEffect(() => {
    if (notice) noticeRef.current?.focus();
  }, [notice]);

  // What the chosen calendar holds already (I4): unchecked when the answer
  // comes, and imported as a copy, under a UID of its own, when checked again.
  const held = useHeldOnImport(calendarId, events);
  const isHeld = (index: number) => held.status === 'known' && held.held.has(index);
  // Each answer is taken once: its rows unchecked, and the rows unchecked for
  // the calendar asked before checked again where this one does not hold them.
  const lastAnswer = useRef<typeof held | null>(null);
  const uncheckedRows = useRef<ReadonlySet<number>>(new Set());
  useEffect(() => {
    if (held.status === 'checking' || lastAnswer.current === held) return;
    lastAnswer.current = held;
    if (held.status === 'failed') {
      announce(t('fileImport.held.checkFailed', { calendar: calendarName }));
      return;
    }
    const before = uncheckedRows.current;
    uncheckedRows.current = held.held;
    setChosen((prev) => rechosenForHeld(prev, before, held.held));
    if (held.held.size > 0) announce(heldSentence(events, held.held, calendarName, t));
  }, [held, announce, calendarName, events, t]);

  const heldStatus = heldStatusSentence(events, held, calendarName, t);

  const toImport = events
    .map((event, index) => ({ event, index }))
    .filter(({ index }) => chosen.has(index) && !imported.has(index));

  const markImported = useCallback((index: number) => {
    setImported((prev) => new Set(prev).add(index));
  }, []);

  // Whose each one is, as the chosen calendar tells: the account's own meeting
  // is a new appointment, someone else's an invitation the calendar keeps or
  // cannot keep quietly (I6).
  const calendar = targets.find((c) => c.id === calendarId);
  const own = useCalendarOwnAddress(
    calendarId,
    events.some((event) => event.organizer?.trim()),
  );
  const intoOf = (event: ImportedEvent) => importInto(importOwner(event, own), calendar);
  const invitationSentence = invitationListSentence(events.map(intoOf), calendarName, t);
  const summary = [
    t('fileImport.list.summary', { file: fileName, count: events.length }),
    skippedSentence(read.skipped, t),
    invitationSentence,
  ]
    .filter(Boolean)
    .join(' ');

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
      onImported: () => markImported(index),
    });
  };

  const importChosen = async () => {
    if (busy) return;
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
    let done = 0;
    // One after the other: a provider asked many times at once may turn the
    // rest away as busy.
    for (const { event: entry, index } of toImport) {
      try {
        await apiCreateEvent(importedCreate(entry, intoOf(entry), calendarId, isHeld(index)), {
          preserveRecurrenceZone: true,
        });
        done += 1;
        markImported(index);
      } catch (err) {
        failed.push({ title: entry.title, err });
      }
    }
    setBusy(false);
    if (done > 0) {
      writeLastUsedCalendar(calendarId);
      invalidateData();
    }
    if (failed.length === 0) {
      announce(t('fileImport.list.done', { count: done, calendar: calendarName }));
      onClose();
      return;
    }
    setNotice(
      t('fileImport.list.partly', {
        done,
        count: toImport.length,
        calendar: calendarName,
        titles: joinNames(
          failed.map((f) => t('fileImport.list.quoted', { title: f.title })),
          t,
        ),
        reason: writeErrorMessage(failed[0].err, t),
      }),
    );
  };

  return (
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
          <FocusableNote ref={noticeRef} className="form__hint form__hint--warning">
            {notice}
          </FocusableNote>
        )}
        <div className="form__field">
          <label className="form__label" htmlFor={calendarFieldId}>
            {t('fileImport.list.calendar')}
          </label>
          <select
            id={calendarFieldId}
            value={calendarId}
            onChange={(e) => setCalendarId(e.target.value)}
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
                  <span className="import-list__done">
                    {`${event.title}, ${when(event)}, ${t('fileImport.list.imported')}`}
                  </span>
                ) : (
                  <>
                    <input
                      id={boxId}
                      type="checkbox"
                      checked={chosen.has(index)}
                      onChange={(e) => {
                        const next = new Set(chosen);
                        if (e.target.checked) next.add(index);
                        else next.delete(index);
                        setChosen(next);
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
  );
}
