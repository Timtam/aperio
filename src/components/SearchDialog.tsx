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
import {
  search as apiSearch,
  isCommandError,
  type EventTypeFilter,
  type SearchKind,
} from '../api/client';
import type { CalendarEvent, Task, TaskStatus } from '../api/types';
import { useCalendarStore } from '../state/calendarStoreContext';
import { useDateFormat } from '../intl/dateFormat';
import { useDialogState } from '../state/dialogStateContext';
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

/**
 * What the user last searched for, kept across the dialog being taken down.
 *
 * The dialog host renders exactly ONE dialog: opening a hit replaces this
 * dialog with the editor, and closing the editor mounts a FRESH one. Without
 * this, coming back from an appointment you just edited meant an empty search
 * field and an empty result list — the work of finding the thing again, every
 * time, which is worst for someone who navigates by keyboard and hears the
 * list rather than seeing it.
 *
 * Module scope, because at most one search dialog exists at a time and the
 * value has no business in the app's state tree. The QUERY and the FILTERS are
 * kept; the hits are not — they are re-fetched on mount, so what comes back
 * reflects the edit that was just made rather than the list as it was before.
 */
const lastSearch: {
  query: string;
  kind: SearchKind;
  calendarIds: Set<string>;
  listIds: Set<string>;
  since: string;
  until: string;
  eventType: EventTypeFilter;
  taskStatuses: Set<TaskStatus>;
} = {
  query: '',
  kind: 'both',
  calendarIds: new Set(),
  listIds: new Set(),
  since: '',
  until: '',
  eventType: 'any',
  taskStatuses: new Set(),
};

/** Forget it — a dialog CLOSED by the user starts clean next time. */
function forgetLastSearch(): void {
  lastSearch.query = '';
  lastSearch.kind = 'both';
  lastSearch.calendarIds = new Set();
  lastSearch.listIds = new Set();
  lastSearch.since = '';
  lastSearch.until = '';
  lastSearch.eventType = 'any';
  lastSearch.taskStatuses = new Set();
}

export function SearchDialog({ isOpen, onClose }: SearchDialogProps) {
  // The user closing the dialog means they are done searching; opening a hit
  // does not, and that is the difference this keeps. Wrapped once so both the
  // Escape/backdrop path and the button below forget the same way.
  const closeAndForget = useCallback(() => {
    forgetLastSearch();
    onClose();
  }, [onClose]);
  const { t } = useTranslation();
  const fmt = useDateFormat();
  const { calendars, taskLists } = useCalendarStore();
  const { openEventDialog, openTaskDialog } = useDialogState();

  // Seeded from the last search so opening a hit and coming back lands where
  // the user left off — see `lastSearch`.
  const [query, setQuery] = useState(() => lastSearch.query);
  const [kind, setKind] = useState<SearchKind>(() => lastSearch.kind);
  const [selectedCalendarIds, setSelectedCalendarIds] = useState<Set<string>>(
    () => new Set(lastSearch.calendarIds),
  );
  const [selectedListIds, setSelectedListIds] = useState<Set<string>>(
    () => new Set(lastSearch.listIds),
  );
  const [since, setSince] = useState(() => lastSearch.since);
  const [until, setUntil] = useState(() => lastSearch.until);
  const [eventType, setEventType] = useState<EventTypeFilter>(
    () => lastSearch.eventType,
  );
  const [selectedTaskStatuses, setSelectedTaskStatuses] = useState<
    Set<TaskStatus>
  >(() => new Set(lastSearch.taskStatuses));
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
          date: fmt.format(new Date(ev.start), 'PPP'),
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

  // Debounced search. Re-fires whenever the query or any filter
  // changes — the user sees the result list narrow immediately when
  // they tick a calendar or flip the kind toggle.
  const lastQueryRef = useRef('');
  const filterKey = useMemo(
    () =>
      kind +
      '|' +
      [...selectedCalendarIds].sort().join(',') +
      '|' +
      [...selectedListIds].sort().join(',') +
      '|' +
      since +
      '|' +
      until +
      '|' +
      eventType +
      '|' +
      [...selectedTaskStatuses].sort().join(','),
    [
      kind,
      selectedCalendarIds,
      selectedListIds,
      since,
      until,
      eventType,
      selectedTaskStatuses,
    ],
  );
  // Remember what is being searched for, for the next time this dialog is
  // mounted (see `lastSearch`). Written on every change rather than on close,
  // because opening a hit takes the dialog down without closing it.
  useEffect(() => {
    lastSearch.query = query;
    lastSearch.kind = kind;
    lastSearch.calendarIds = new Set(selectedCalendarIds);
    lastSearch.listIds = new Set(selectedListIds);
    lastSearch.since = since;
    lastSearch.until = until;
    lastSearch.eventType = eventType;
    lastSearch.taskStatuses = new Set(selectedTaskStatuses);
  }, [
    query,
    kind,
    selectedCalendarIds,
    selectedListIds,
    since,
    until,
    eventType,
    selectedTaskStatuses,
  ]);

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
      apiSearch(trimmed, {
        kind,
        calendar_ids:
          selectedCalendarIds.size > 0 ? [...selectedCalendarIds] : undefined,
        list_ids:
          selectedListIds.size > 0 ? [...selectedListIds] : undefined,
        since: since ? `${since}T00:00:00Z` : undefined,
        until: until ? `${until}T23:59:59Z` : undefined,
        event_type: eventType,
        task_statuses:
          selectedTaskStatuses.size > 0
            ? [...selectedTaskStatuses]
            : undefined,
      })
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
    // filterKey captures everything below — but we list the originals
    // so React's dep-check stays happy with exhaustive-deps.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [query, filterKey]);

  // Clamp focus when the result list shrinks.
  useEffect(() => {
    if (focusIndex >= hits.length) {
      setFocusIndex(Math.max(0, hits.length - 1));
    }
  }, [hits.length, focusIndex]);

  const openHit = useCallback(
    (hit: Hit) => {
      // Push the edit dialog on top of the search results; closing
      // it pops back here so the user can keep clicking through
      // hits without re-running the query. The dialog stack in
      // DialogState owns the restore-focus logic.
      if (hit.kind === 'event') {
        const ev = events.find((e) => e.id === hit.id);
        openEventDialog(ev ?? null);
      } else {
        const task = tasks.find((tk) => tk.id === hit.id);
        openTaskDialog(task ?? null);
      }
    },
    [events, tasks, openEventDialog, openTaskDialog],
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
      onClose={closeAndForget}
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

        <details className="search-filters">
          <summary>{t('dialogs.search.filtersTitle')}</summary>
          <fieldset className="form__field">
            <legend className="form__label">
              {t('dialogs.search.kindLabel')}
            </legend>
            {(['both', 'events', 'tasks'] as const).map((k) => (
              <label
                key={k}
                className="form__field form__field--inline"
              >
                <input
                  type="radio"
                  name="search-kind"
                  checked={kind === k}
                  onChange={() => setKind(k)}
                />
                <span>{t(`dialogs.search.kind.${k}`)}</span>
              </label>
            ))}
          </fieldset>

          {kind !== 'tasks' && calendars.length > 0 && (
            <fieldset className="form__field">
              <legend className="form__label">
                {t('dialogs.search.calendarsLabel')}
              </legend>
              <p className="form__hint">
                {t('dialogs.search.containersHint')}
              </p>
              {calendars.map((c) => (
                <label
                  key={c.id}
                  className="form__field form__field--inline"
                >
                  <input
                    type="checkbox"
                    checked={selectedCalendarIds.has(c.id)}
                    onChange={(e) => {
                      setSelectedCalendarIds((prev) => {
                        const next = new Set(prev);
                        if (e.target.checked) next.add(c.id);
                        else next.delete(c.id);
                        return next;
                      });
                    }}
                  />
                  <span>{c.name}</span>
                </label>
              ))}
            </fieldset>
          )}

          {kind !== 'events' && taskLists.length > 0 && (
            <fieldset className="form__field">
              <legend className="form__label">
                {t('dialogs.search.listsLabel')}
              </legend>
              <p className="form__hint">
                {t('dialogs.search.containersHint')}
              </p>
              {taskLists.map((l) => (
                <label
                  key={l.id}
                  className="form__field form__field--inline"
                >
                  <input
                    type="checkbox"
                    checked={selectedListIds.has(l.id)}
                    onChange={(e) => {
                      setSelectedListIds((prev) => {
                        const next = new Set(prev);
                        if (e.target.checked) next.add(l.id);
                        else next.delete(l.id);
                        return next;
                      });
                    }}
                  />
                  <span>{l.name}</span>
                </label>
              ))}
            </fieldset>
          )}

          <fieldset className="form__field">
            <legend className="form__label">
              {t('dialogs.search.rangeLabel')}
            </legend>
            <div className="form__row">
              <label className="form__field">
                <span className="form__label">
                  {t('dialogs.search.sinceLabel')}
                </span>
                <input
                  type="date"
                  value={since}
                  onChange={(e) => setSince(e.target.value)}
                />
              </label>
              <label className="form__field">
                <span className="form__label">
                  {t('dialogs.search.untilLabel')}
                </span>
                <input
                  type="date"
                  value={until}
                  onChange={(e) => setUntil(e.target.value)}
                />
              </label>
            </div>
          </fieldset>

          {kind !== 'tasks' && (
            <fieldset className="form__field">
              <legend className="form__label">
                {t('dialogs.search.eventTypeLabel')}
              </legend>
              {(['any', 'single', 'recurring', 'all_day'] as const).map(
                (etype) => (
                  <label
                    key={etype}
                    className="form__field form__field--inline"
                  >
                    <input
                      type="radio"
                      name="search-event-type"
                      checked={eventType === etype}
                      onChange={() => setEventType(etype)}
                    />
                    <span>{t(`dialogs.search.eventType.${etype}`)}</span>
                  </label>
                ),
              )}
            </fieldset>
          )}

          {kind !== 'events' && (
            <fieldset className="form__field">
              <legend className="form__label">
                {t('dialogs.search.taskStatusLabel')}
              </legend>
              <p className="form__hint">
                {t('dialogs.search.containersHint')}
              </p>
              {(
                ['open', 'in_progress', 'completed', 'cancelled'] as const
              ).map((status) => (
                <label
                  key={status}
                  className="form__field form__field--inline"
                >
                  <input
                    type="checkbox"
                    checked={selectedTaskStatuses.has(status)}
                    onChange={(e) => {
                      setSelectedTaskStatuses((prev) => {
                        const next = new Set(prev);
                        if (e.target.checked) next.add(status);
                        else next.delete(status);
                        return next;
                      });
                    }}
                  />
                  <span>{t(`dialogs.task.status.${statusKey(status)}`)}</span>
                </label>
              ))}
            </fieldset>
          )}
        </details>

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
          <button type="button" onClick={closeAndForget} className="form__action">
            {t('dialogs.close')}
          </button>
        </div>
      </div>
    </Modal>
  );
}

/** Map a TaskStatus value to the camelCase key used by the task dialog
 *  translations (e.g. `in_progress` → `inProgress`). */
function statusKey(status: TaskStatus): string {
  return status === 'in_progress' ? 'inProgress' : status;
}
