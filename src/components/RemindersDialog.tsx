import {
  useCallback,
  useEffect,
  useId,
  useMemo,
  useState,
} from 'react';
import { useTranslation } from 'react-i18next';

import { useAutoFocus } from '../hooks/useAutoFocus';
import {
  getEventById,
  getTaskById,
  isCommandError,
  listUpcomingReminders,
  type UpcomingReminder,
} from '../api/client';
import { useDateFormat } from '../intl/dateFormat';
import { useDialogState } from '../state/DialogState';
import { Modal } from './Modal';

/**
 * Reminders overview dialog (DESIGN.md section 14.6, `Ctrl+Shift+R`).
 *
 * Shows the chronologically sorted list of upcoming reminder triggers
 * that the local scheduler currently has on its plan. Enter on a row
 * opens the underlying event or task in its full edit dialog so the
 * user can adjust the reminder there.
 *
 * Listbox + aria-activedescendant matches the rest of the app's
 * keyboard-first navigation: NVDA stays in focus mode, arrow keys
 * walk the list without ever moving DOM focus, and Enter / Space
 * actuates the focused row.
 */
export interface RemindersDialogProps {
  isOpen: boolean;
  onClose: () => void;
}

export function RemindersDialog({ isOpen, onClose }: RemindersDialogProps) {
  const { t } = useTranslation();
  const fmt = useDateFormat();
  const { openEventDialog, openTaskDialog } = useDialogState();

  const [items, setItems] = useState<UpcomingReminder[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [focusIndex, setFocusIndex] = useState(0);

  // Re-fetch whenever the dialog reopens so the list reflects any
  // changes made since the last view.
  useEffect(() => {
    if (!isOpen) return;
    let cancelled = false;
    setLoading(true);
    setError(null);
    listUpcomingReminders()
      .then((res) => {
        if (cancelled) return;
        setItems(res);
        setFocusIndex(0);
      })
      .catch((err) => {
        if (cancelled) return;
        if (isCommandError(err)) {
          setError(`${err.code}: ${err.message}`);
        } else {
          setError(String(err));
        }
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [isOpen]);

  useEffect(() => {
    if (focusIndex >= items.length) {
      setFocusIndex(Math.max(0, items.length - 1));
    }
  }, [items.length, focusIndex]);

  const idPrefix = useId();
  const itemId = (i: number) => `${idPrefix}-r-${i}`;
  const listRef = useAutoFocus<HTMLUListElement>(!loading);

  const rows = useMemo(
    () =>
      items.map((r) => ({
        ...r,
        when: fmt.format(new Date(r.trigger_at), 'PPpp'),
      })),
    [items, fmt],
  );

  const openRow = useCallback(
    async (r: UpcomingReminder) => {
      // Fetch the full row by id then push the matching edit dialog
      // on top of this one. Closing the edit dialog will pop us back
      // to the overview, where the user can keep working through the
      // list. We deliberately do NOT call `onClose` first — that
      // would empty the stack and the user would land in the shell.
      try {
        if (r.item_kind === 'event') {
          const ev = await getEventById(r.item_id);
          openEventDialog(ev ?? null);
        } else {
          const task = await getTaskById(r.item_id);
          openTaskDialog(task ?? null);
        }
      } catch (err) {
        // eslint-disable-next-line no-console
        console.warn('failed to load reminder target', err);
      }
    },
    [openEventDialog, openTaskDialog],
  );

  const handleKey = (e: React.KeyboardEvent<HTMLUListElement>) => {
    if (e.ctrlKey || e.metaKey || e.altKey) return;
    if (rows.length === 0) return;
    switch (e.key) {
      case 'ArrowDown':
        e.preventDefault();
        setFocusIndex((i) => Math.min(i + 1, rows.length - 1));
        return;
      case 'ArrowUp':
        e.preventDefault();
        setFocusIndex((i) => Math.max(i - 1, 0));
        return;
      case 'Home':
        e.preventDefault();
        setFocusIndex(0);
        return;
      case 'End':
        e.preventDefault();
        setFocusIndex(rows.length - 1);
        return;
      case 'Enter':
      case ' ':
      case 'Spacebar':
        e.preventDefault();
        if (rows[focusIndex]) openRow(items[focusIndex]);
        return;
      default:
        return;
    }
  };

  return (
    <Modal
      isOpen={isOpen}
      onClose={onClose}
      title={t('dialogs.reminders.title')}
      className="modal--form"
    >
      <div className="form">
        <p
          className="sr-only"
          aria-live="polite"
          aria-atomic="true"
        >
          {loading
            ? t('dialogs.reminders.loading')
            : t('dialogs.reminders.count', { count: rows.length })}
        </p>

        {error && (
          <p role="alert" className="form__error">
            {error}
          </p>
        )}

        {rows.length === 0 && !loading && !error && (
          <p className="form__hint">{t('dialogs.reminders.empty')}</p>
        )}

        {rows.length > 0 && (
          <ul
            ref={listRef}
            role="listbox"
            tabIndex={0}
            aria-label={t('dialogs.reminders.listLabel')}
            aria-activedescendant={itemId(focusIndex)}
            onKeyDown={handleKey}
            className="reminders-list"
          >
            {rows.map((r, i) => {
              const focused = i === focusIndex;
              return (
                <li
                  key={`${r.item_kind}-${r.item_id}-${r.trigger_at}-${i}`}
                  id={itemId(i)}
                  role="option"
                  aria-selected={focused}
                  aria-label={t(
                    r.item_kind === 'event'
                      ? 'dialogs.reminders.eventRow'
                      : 'dialogs.reminders.taskRow',
                    { title: r.title, when: r.when },
                  )}
                  className={
                    'reminders-list__item' +
                    (focused ? ' reminders-list__item--focused' : '')
                  }
                  onClick={() => {
                    setFocusIndex(i);
                    openRow(items[i]);
                  }}
                >
                  <span className="reminders-list__when">{r.when}</span>
                  <span className="reminders-list__title">{r.title}</span>
                  <span className="reminders-list__kind">
                    {t(
                      r.item_kind === 'event'
                        ? 'dialogs.reminders.kindEvent'
                        : 'dialogs.reminders.kindTask',
                    )}
                  </span>
                </li>
              );
            })}
          </ul>
        )}

        <div className="form__actions">
          <button type="button" onClick={onClose} className="form__action">
            {t('dialogs.close')}
          </button>
        </div>
      </div>
    </Modal>
  );
}
