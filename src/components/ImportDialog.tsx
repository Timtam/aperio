import { useCallback, useEffect, useId, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  importedForEditor,
  importFailureSentence,
  joinNames,
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

  const toImport = events
    .map((event, index) => ({ event, index }))
    .filter(({ index }) => chosen.has(index) && !imported.has(index));

  const markImported = useCallback((index: number) => {
    setImported((prev) => new Set(prev).add(index));
  }, []);

  const setAside = events.some((event) => importedForEditor(event).invitationSetAside);
  const summary = [
    t('fileImport.list.summary', { file: fileName, count: events.length }),
    skippedSentence(read.skipped, t),
    setAside ? t('fileImport.invitationsSetAside') : null,
  ]
    .filter(Boolean)
    .join(' ');

  const when = (event: ImportedEvent): string => {
    const start = new Date(event.start);
    const text = event.all_day
      ? t('fileImport.list.allDay', { date: fmt.format(start, 'PPP') })
      : t('fileImport.list.timed', {
          date: fmt.format(start, 'PPP'),
          time: `${fmt.format(start, 'p')}–${fmt.format(new Date(event.end), 'p')}`,
        });
    return event.recurrence ? `${text}, ${t('fileImport.list.series')}` : text;
  };

  const edit = (event: ImportedEvent, index: number) => {
    const { event: forEditor, invitationSetAside } = importedForEditor(event);
    openEventDialog(null, {
      calendarId,
      importFrom: forEditor,
      importNote: invitationSetAside ? t('fileImport.invitationSetAside') : null,
      onImported: () => markImported(index),
    });
  };

  const importChosen = async () => {
    if (busy) return;
    if (toImport.length === 0) {
      setNotice(t('fileImport.list.noneChosen'));
      return;
    }
    setBusy(true);
    setNotice(null);
    const failed: { title: string; err: unknown }[] = [];
    let done = 0;
    // One after the other: a provider asked many times at once may turn the
    // rest away as busy.
    for (const { event, index } of toImport) {
      const { event: entry } = importedForEditor(event);
      try {
        await apiCreateEvent(
          {
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
            attendees: entry.attendees,
            send_invitations: false,
            // No alarm in the file: the calendar's defaults, as for a new
            // appointment nobody chose reminders for.
            use_calendar_defaults: entry.reminders.length === 0,
          },
          { preserveRecurrenceZone: true },
        );
        done += 1;
        markImported(index);
      } catch (err) {
        failed.push({ title: event.title, err });
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
                    <label htmlFor={boxId}>{`${event.title}, ${when(event)}`}</label>
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
