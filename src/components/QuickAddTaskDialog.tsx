import {
  useCallback,
  useEffect,
  useMemo,
  useState,
  type FormEvent,
} from 'react';
import { useTranslation } from 'react-i18next';

import { useAnnouncer } from '../a11y/announcerContext';
import { createTask as apiCreateTask, isCommandError } from '../api/client';
import { useCalendarStore } from '../state/calendarStoreContext';
import { useDialogState } from '../state/dialogStateContext';
import { readLastUsedTaskList, writeLastUsedTaskList } from './lastUsedTaskList';
import { Modal } from './Modal';

/**
 * Quick-add *task* dialog — the task counterpart of {@link QuickAddDialog}.
 *
 * Minimal form for one-tap task capture: title, an optional scheduled day
 * (empty by default → the task lands in the backlog), and the list. "More
 * details" swaps to the full TaskDialog, carrying the in-progress values
 * along. Opened via `Alt+N` and the toolbar.
 */
export function QuickAddTaskDialog({
  isOpen,
  onClose,
  defaultDate,
}: {
  isOpen: boolean;
  onClose: () => void;
  /** YYYY-MM-DD to schedule the new task on (an activated calendar day).
   *  When omitted the task starts dateless (backlog). */
  defaultDate?: string;
}) {
  const { t } = useTranslation();
  const announce = useAnnouncer();
  const { taskLists } = useCalendarStore();
  const { openTaskDialog } = useDialogState();

  const initial = useMemo(
    () => buildInitial(taskLists, defaultDate),
    [taskLists, defaultDate],
  );

  const [title, setTitle] = useState(initial.title);
  const [date, setDate] = useState(initial.date);
  const [listId, setListId] = useState(initial.listId);
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  useEffect(() => {
    if (isOpen) {
      setTitle(initial.title);
      setDate(initial.date);
      setListId(initial.listId);
      setError(null);
    }
  }, [isOpen, initial]);

  const onSubmit = useCallback(
    async (e: FormEvent) => {
      e.preventDefault();
      const trimmed = title.trim();
      if (!trimmed) {
        setError(t('dialogs.task.titleRequired'));
        return;
      }
      if (!listId) {
        setError(t('dialogs.task.listRequired'));
        return;
      }

      setSubmitting(true);
      try {
        await apiCreateTask({
          list_id: listId,
          title: trimmed,
          description: null,
          status: 'open',
          priority: 'medium',
          // Empty date → backlog; a chosen day schedules it.
          scheduled_date: date || null,
          scheduled_time: null,
          deadline_date: null,
          deadline_time: null,
          recurrence: null,
          parent_id: null,
          section_id: null,
          color_label: null,
          reminders: [],
          assignees: [],
          sound: null,
        });
        writeLastUsedTaskList(listId);
        announce(t('dialogs.task.created', { title: trimmed }));
        onClose();
      } catch (err) {
        if (isCommandError(err)) {
          setError(`${err.code}: ${err.message}`);
        } else {
          setError(String(err));
        }
      } finally {
        setSubmitting(false);
      }
    },
    [title, listId, date, announce, onClose, t],
  );

  const openFullDialog = useCallback(() => {
    onClose();
    openTaskDialog(null, {
      listId: listId || undefined,
      defaultDate: date || undefined,
      // Carry the in-progress title over so it isn't lost on the hand-off.
      defaultTitle: title || undefined,
    });
  }, [onClose, openTaskDialog, listId, date, title]);

  const writableLists = taskLists.filter((l) => !l.read_only);

  return (
    <Modal
      isOpen={isOpen}
      onClose={onClose}
      title={t('dialogs.quickAddTask.title')}
      className="modal--form modal--narrow"
      dismissOnBackdrop={false}
    >
      <form onSubmit={(e) => void onSubmit(e)} className="form">
        <label className="form__field">
          <span className="form__label">
            {t('dialogs.task.fields.title')}
          </span>
          <input
            type="text"
            value={title}
            onChange={(e) => setTitle(e.target.value)}
            required
            autoComplete="off"
          />
        </label>

        <label className="form__field">
          <span className="form__label">
            {t('dialogs.task.fields.scheduled.legend')}
          </span>
          <input
            type="date"
            value={date}
            onChange={(e) => setDate(e.target.value)}
          />
        </label>

        <label className="form__field">
          <span className="form__label">
            {t('dialogs.task.fields.list')}
          </span>
          <select
            value={listId}
            onChange={(e) => setListId(e.target.value)}
            required
          >
            <option value="" disabled>
              {t('dialogs.task.pickList')}
            </option>
            {writableLists.map((list) => (
              <option key={list.id} value={list.id}>
                {list.name}
              </option>
            ))}
          </select>
        </label>

        {error && (
          <p role="alert" className="form__error">
            {error}
          </p>
        )}

        <div className="form__actions">
          <button
            type="button"
            onClick={openFullDialog}
            aria-disabled={submitting || undefined}
            className="form__action"
          >
            {t('dialogs.quickAdd.moreDetails')}
          </button>
          <button
            type="button"
            onClick={onClose}
            aria-disabled={submitting || undefined}
            className="form__action"
          >
            {t('dialogs.cancel')}
          </button>
          <button
            type="submit"
            aria-disabled={submitting || undefined}
            className="form__action form__action--primary"
          >
            {t('dialogs.create')}
          </button>
        </div>
      </form>
    </Modal>
  );
}

interface Initial {
  title: string;
  date: string;
  listId: string;
}

function buildInitial(
  taskLists: { id: string; read_only: boolean }[],
  defaultDate?: string,
): Initial {
  // Default list mirrors TaskDialog: last-used (if still present + writable),
  // else the first writable list, else any list.
  const writable = taskLists.filter((l) => !l.read_only);
  const lastUsed = readLastUsedTaskList();
  const lastUsedValid =
    lastUsed && writable.some((l) => l.id === lastUsed) ? lastUsed : null;
  return {
    title: '',
    // Dateless by default → backlog; an activated calendar day schedules it.
    date: defaultDate ?? '',
    listId: lastUsedValid ?? writable[0]?.id ?? taskLists[0]?.id ?? '',
  };
}
