import {
  useCallback,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
} from 'react';
import { useTranslation } from 'react-i18next';

import { useAutoFocus } from '../hooks/useAutoFocus';
import { search as apiSearch, isCommandError } from '../api/client';
import type { CalendarEvent, Task } from '../api/types';
import { useCalendarStore } from '../state/CalendarStore';
import { useDateFormat } from '../intl/dateFormat';
import { useDialogState } from '../state/DialogState';
import { Modal } from './Modal';

/**
 * Search dialog (DESIGN.md section 13).
 *
 * A live-search field on top, a listbox of hits below. Pressing Enter
 * on a hit closes the search dialog and opens the matching event or
 * task in its full edit dialog.
 *
 * Phase 4d ships the core flow only. Filters (type / range / list)
 * land in a follow-up — the listbox is already complete enough on its
 * own to find any term across the database.
 *
 * The 200 ms debounce matches the spec's "Ergebnisse erscheinen live
 * während der Eingabe" and keeps the backend out of every keystroke.
 */
export interface SearchDialogProps {
  isOpen: boolean;
  onClose: () => void;
}

interface Hit {
  kind: 'event' | 'task';
  id: string;
  title: string;
  /** Already-localised secondary line (date for events, list for tasks). */
  secondary: string;
}

export function SearchDialog({ isOpen, onClose }: SearchDialogProps) {
  const { t } = useTranslation();
  const fmt = useDateFormat();
  const { calendars, taskLists } = useCalendarStore();
  const { openEventDialog, openTaskDialog } = useDialogState();

  const [query, setQuery] = useState('');
  const [events, setEvents] = useState<CalendarEvent[]>([]);
  const [tasks, setTasks] = useState<Task[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [focusIndex, setFocusIndex] = useState(0);

  const inputRef = useAutoFocus<HTMLInputElement>();
  const idPrefix = useId();
  const optionId = (i: number) => `${idPrefix}-hit-${i}`;

  // Lookup tables for the secondary line.
  const calendarsById = useMemo(() => {
    const m = new Map<string, (typeof calendars)[number]>();
    calendars.forEach((c) => m.set(c.id, c));
    return m;
  }, [calendars]);
  const listsById = useMemo(() => {
    const m = new Map<string, (typeof taskLists)[number]>();
    taskLists.forEach((l) => m.set(l.id, l));
    return m;
  }, [taskLists]);

  // Flat hit list, events first then tasks. Sorted internally by FTS
  // rank, which is good enough for a search-as-you-type experience.
  const hits = useMemo<Hit[]>(() => {
    const out: Hit[] = [];
    events.forEach((ev) => {
      out.push({
        kind: 'event',
        id: ev.id,
        title: ev.title,
        secondary: t('dialogs.search.eventSecondary', {
          date: fmt.format(new Date(ev.start), 'PP'),
          calendar: calendarsById.get(ev.calendar_id)?.name ?? '—',
        }),
      });
    });
    tasks.forEach((task) => {
      out.push({
        kind: 'task',
        id: task.id,
        title: task.title,
        secondary: t('dialogs.search.taskSecondary', {
          list: listsById.get(task.list_id)?.name ?? '—',
        }),
      });
    });
    return out;
  }, [events, tasks, t, fmt, calendarsById, listsById]);

  // Debounced search.
  const lastQueryRef = useRef('');
  useEffect(() => {
    const trimmed = query.trim();
    lastQueryRef.current = trimmed;
    if (trimmed.length === 0) {
      setEvents([]);
      setTasks([]);
      setLoading(false);
      return;
    }
    setLoading(true);
    const handle = window.setTimeout(() => {
      apiSearch(trimmed)
        .then((res) => {
          // Skip stale results — another keystroke may have moved on.
          if (lastQueryRef.current !== trimmed) return;
          setEvents(res.events);
          setTasks(res.tasks);
          setError(null);
        })
        .catch((err) => {
          if (lastQueryRef.current !== trimmed) return;
          if (isCommandError(err)) {
            setError(`${err.code}: ${err.message}`);
          } else {
            setError(String(err));
          }
        })
        .finally(() => {
          if (lastQueryRef.current === trimmed) setLoading(false);
        });
    }, 200);
    return () => window.clearTimeout(handle);
  }, [query]);

  // Clamp focus when the result list shrinks.
  useEffect(() => {
    if (focusIndex >= hits.length) {
      setFocusIndex(Math.max(0, hits.length - 1));
    }
  }, [hits.length, focusIndex]);

  const openHit = useCallback(
    (hit: Hit) => {
      if (hit.kind === 'event') {
        const ev = events.find((e) => e.id === hit.id);
        onClose();
        // Defer so the search modal finishes closing before we open
        // the edit one — otherwise the focus restore in DialogState
        // would land on the search box and immediately be lost again.
        queueMicrotask(() => openEventDialog(ev ?? null));
      } else {
        const task = tasks.find((tk) => tk.id === hit.id);
        onClose();
        queueMicrotask(() => openTaskDialog(task ?? null));
      }
    },
    [events, tasks, onClose, openEventDialog, openTaskDialog],
  );

  const handleInputKey = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.ctrlKey || e.metaKey || e.altKey) return;
    if (hits.length === 0) return;
    switch (e.key) {
      case 'ArrowDown':
        e.preventDefault();
        setFocusIndex((i) => Math.min(i + 1, hits.length - 1));
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
        setFocusIndex(hits.length - 1);
        return;
      case 'Enter':
        e.preventDefault();
        if (hits[focusIndex]) openHit(hits[focusIndex]);
        return;
      default:
        return;
    }
  };

  return (
    <Modal
      isOpen={isOpen}
      onClose={onClose}
      title={t('dialogs.search.title')}
      className="modal--form modal--wide"
    >
      <div className="form">
        <label className="form__field">
          <span className="form__label">{t('dialogs.search.field')}</span>
          <input
            ref={inputRef}
            type="text"
            role="searchbox"
            aria-controls={`${idPrefix}-results`}
            aria-activedescendant={
              hits.length > 0 ? optionId(focusIndex) : undefined
            }
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={handleInputKey}
            autoComplete="off"
            spellCheck={false}
            placeholder={t('dialogs.search.placeholder')}
          />
        </label>

        <p
          className="sr-only"
          aria-live="polite"
          aria-atomic="true"
        >
          {loading
            ? t('dialogs.search.searching')
            : query.trim().length === 0
              ? ''
              : t('dialogs.search.resultCount', { count: hits.length })}
        </p>

        {error && (
          <p role="alert" className="form__error">
            {error}
          </p>
        )}

        <ul
          id={`${idPrefix}-results`}
          role="listbox"
          aria-label={t('dialogs.search.results')}
          className="search-results"
        >
          {hits.length === 0 && query.trim().length > 0 && !loading && (
            <li role="presentation" className="search-results__empty">
              {t('dialogs.search.noResults')}
            </li>
          )}
          {hits.map((hit, i) => {
            const focused = i === focusIndex;
            return (
              <li
                key={`${hit.kind}-${hit.id}`}
                id={optionId(i)}
                role="option"
                aria-selected={focused}
                aria-label={t(
                  hit.kind === 'event'
                    ? 'dialogs.search.eventAria'
                    : 'dialogs.search.taskAria',
                  { title: hit.title, secondary: hit.secondary },
                )}
                className={
                  'search-results__item' +
                  (focused ? ' search-results__item--focused' : '')
                }
                onClick={() => {
                  setFocusIndex(i);
                  openHit(hit);
                }}
              >
                <span className="search-results__kind">
                  {hit.kind === 'event'
                    ? t('dialogs.search.kindEvent')
                    : t('dialogs.search.kindTask')}
                </span>
                <span className="search-results__title">{hit.title}</span>
                <span className="search-results__secondary">
                  {hit.secondary}
                </span>
              </li>
            );
          })}
        </ul>

        <div className="form__actions">
          <button type="button" onClick={onClose} className="form__action">
            {t('dialogs.close')}
          </button>
        </div>
      </div>
    </Modal>
  );
}
