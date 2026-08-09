import { useCallback, useEffect, useId, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';

import type { CalendarEvent } from '../../api/types';
import { useAnnouncer } from '../../a11y/announcerContext';
import { useAutoFocus } from '../../hooks/useAutoFocus';
import { useDeferredLoading } from '../../hooks/useDeferredLoading';
import { localDateKey } from '../../intl/dateKey';
import { useDateFormat } from '../../intl/dateFormat';
import { labelsLookup, resolveEventColor } from '../../intl/eventColor';
import {
  isSeriesOccurrence,
  occurrenceIsoOf,
  seriesIdOf,
} from '../../intl/recurrence';
import {
  collapseEventGroups,
  eventInstanceKey,
  type CollapsedRow,
} from '@aperio/shared';
import {
  expandToDayOccurrences,
  type DayOccurrence,
} from '../../intl/multiDay';
import { useCalendarStore } from '../../state/calendarStoreContext';
import { useChipContextMenu } from '../../state/useChipContextMenu';
import { useDialogState } from '../../state/dialogStateContext';
import { useEvents } from '../../state/useEvents';
import { useEventGroups } from '../../state/useEventGroups';
import { useViewState } from '../../state/viewStateContext';
import { visibleRange } from '../../state/viewMath';
import { duplicateEvent } from '../duplicateActions';
import { ConfirmDialog } from '../ConfirmDialog';
import { DeleteEventScopeDialog } from '../DeleteEventScopeDialog';
import {
  addEventExdate,
  deleteEventById,
  isCommandError,
} from '../../api/client';
import { deleteThisAndFuture } from '../../state/deleteSeriesFromOccurrence';

/**
 * Agenda — flat, chronologically ordered listbox of events with visual
 * day separators.
 *
 * DESIGN.md section 3.3 calls for a `role="list"` with nested
 * `role="listitem"` for day groups, but a content list isn't navigable
 * by arrow keys and tabIndex=-1 on the surrounding section made NVDA
 * drop out of focus mode. Listbox + `aria-activedescendant` matches the
 * keyboard model the spec asks for (Arrow Up/Down between events) and
 * keeps the screen reader in focus mode end-to-end.
 *
 * Date context isn't lost: the day label is folded into the option's
 * `aria-label`, so jumping into the middle of a group still announces
 * the date. The visual day separators between options are
 * `aria-hidden` — purely sighted-user affordance.
 */
export function AgendaView() {
  const { t } = useTranslation();
  const announce = useAnnouncer();
  const { anchor } = useViewState();
  const fmt = useDateFormat();
  const { openEventDialog, openMoveCopy, invalidateData } = useDialogState();

  const range = useMemo(() => visibleRange('agenda', anchor), [anchor]);
  const { events, calendarById, loading } = useEvents(range);
  const { openForEvent: openEventMenu } = useChipContextMenu();
  const { colorLabels } = useCalendarStore();
  const labelById = useMemo(() => labelsLookup(colorLabels), [colorLabels]);

  // Expand multi-day all-day events into one renderable row per
  // covered day. A 14-day vacation becomes 14 rows, each carrying
  // "Tag 3 von 14" position info; keyboard focus / Enter / Delete
  // operate on the underlying event via `occurrences[focusIndex].ev`.
  const allOccurrences = useMemo(
    () => expandToDayOccurrences(events, range),
    [events, range],
  );
  const groups = useEventGroups(events);
  /**
   * One row per appointment instead of one per copy — folded PER DAY, which
   * is the contract `collapseEventGroups` documents: a recurring appointment
   * renders a row per day, so judged across the whole agenda its own days
   * would look exactly like copies that disagree.
   */
  const { occurrences, groupRows } = useMemo(() => {
    const byDay = new Map<string, DayOccurrence[]>();
    for (const occ of allOccurrences) {
      const key = localDateKey(occ.day);
      byDay.set(key, [...(byDay.get(key) ?? []), occ]);
    }
    const kept: DayOccurrence[] = [];
    const rows = new Map<string, CollapsedRow<CalendarEvent>>();
    for (const [dayKey, dayOccurrences] of byDay) {
      const folded = collapseEventGroups(
        dayOccurrences.map((o) => o.ev),
        groups,
        seriesIdOf,
      );
      const survivors = new Set(folded.map((row) => eventInstanceKey(row.event)));
      for (const row of folded) {
        rows.set(`${eventInstanceKey(row.event)}@${dayKey}`, row);
      }
      for (const occ of dayOccurrences) {
        if (survivors.has(eventInstanceKey(occ.ev))) kept.push(occ);
      }
    }
    return { occurrences: kept, groupRows: rows };
  }, [allOccurrences, groups]);

  const [focusIndex, setFocusIndex] = useState(0);

  useEffect(() => {
    if (focusIndex >= occurrences.length) {
      setFocusIndex(Math.max(0, occurrences.length - 1));
    }
  }, [occurrences.length, focusIndex]);

  // Deferred indicator — see DayView for the full rationale.
  const showLoading = useDeferredLoading(loading);
  useEffect(() => {
    if (showLoading) announce(t('views.loading'));
  }, [showLoading, announce, t]);

  const idPrefix = useId();
  const itemId = useCallback(
    (i: number) => `${idPrefix}-item-${i}`,
    [idPrefix],
  );
  const listRef = useAutoFocus<HTMLUListElement>(!loading);

  const [confirmTarget, setConfirmTarget] = useState<CalendarEvent | null>(
    null,
  );
  const [scopeTarget, setScopeTarget] = useState<CalendarEvent | null>(null);

  const performDelete = useCallback(
    async (
      ev: CalendarEvent,
      scope: 'occurrence' | 'this_and_future' | 'series',
      sendCancellations = false,
    ) => {
      try {
        const occIso = occurrenceIsoOf(ev);
        if (scope === 'occurrence' && occIso) {
          await addEventExdate(seriesIdOf(ev), occIso, ev.calendar_id, sendCancellations);
          announce(
            t(
              sendCancellations
                ? 'dialogs.event.occurrenceCancelled'
                : 'dialogs.event.occurrenceDeleted',
              { title: ev.title },
            ),
          );
        } else if (scope === 'this_and_future' && occIso) {
          await deleteThisAndFuture(ev, occIso, sendCancellations);
          announce(
            t(
              sendCancellations
                ? 'dialogs.event.thisAndFutureCancelled'
                : 'dialogs.event.thisAndFutureDeleted',
              { title: ev.title },
            ),
          );
        } else {
          await deleteEventById(seriesIdOf(ev), ev.calendar_id, sendCancellations);
          announce(
            t(
              sendCancellations
                ? 'dialogs.event.meetingCancelled'
                : 'dialogs.event.deleted',
              { title: ev.title },
            ),
          );
        }
        // Local view-state dialogs don't go through DialogState.close(),
        // so the dataVersion bump has to be explicit here.
        invalidateData();
      } catch (err) {
        if (isCommandError(err)) {
          announce(`${err.code}: ${err.message}`);
        } else {
          announce(String(err));
        }
      }
    },
    [announce, t, invalidateData],
  );

  const requestDelete = useCallback((ev: CalendarEvent) => {
    // Recurring-aware delete prompt: ask "this occurrence vs whole
    // Only a synthetic EXPANDED occurrence has a single instance to delete, so
    // only it gets the occurrence-vs-series choice. A bare recurring master row
    // (an unexpandable RRULE) has no single occurrence — offering "this
    // occurrence" there would fall through to a full-series delete — so it takes
    // the plain confirm (which deletes the series).
    if (isSeriesOccurrence(ev)) {
      setScopeTarget(ev);
    } else {
      setConfirmTarget(ev);
    }
  }, []);

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      const ev = occurrences[focusIndex]?.ev;
      if (e.ctrlKey || e.metaKey) {
        if (e.key.toLowerCase() === 'd' && !e.shiftKey && !e.altKey) {
          e.preventDefault();
          if (ev) {
            void duplicateEvent(ev).then(() =>
              announce(t('actions.duplicated', { title: ev.title })),
            );
          }
        }
        return;
      }
      if (e.altKey) return;
      if (e.shiftKey && e.key.toLowerCase() === 'm') {
        e.preventDefault();
        if (ev) openMoveCopy({ kind: 'event', event: ev });
        return;
      }
      if (occurrences.length === 0) return;
      switch (e.key) {
        case 'ArrowDown':
          e.preventDefault();
          setFocusIndex((i) => Math.min(i + 1, occurrences.length - 1));
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
          setFocusIndex(occurrences.length - 1);
          return;
        case 'Enter':
        case ' ':
        case 'Spacebar': {
          e.preventDefault();
          if (ev) openEventDialog(ev);
          return;
        }
        case 'Delete':
        case 'Backspace': {
          e.preventDefault();
          if (ev) requestDelete(ev);
          return;
        }
        case 'ContextMenu':
        case 'F10': {
          if (e.key === 'F10' && !e.shiftKey) return;
          e.preventDefault();
          if (ev) {
            const target = e.currentTarget as HTMLElement;
            const id = itemId(focusIndex);
            const node = target.ownerDocument?.getElementById(id);
            const rect = node?.getBoundingClientRect();
            const pos = rect
              ? { x: rect.left, y: rect.bottom }
              : undefined;
            void openEventMenu(ev, pos);
          }
          return;
        }
        default:
          return;
      }
    },
    [
      occurrences,
      focusIndex,
      openEventDialog,
      openMoveCopy,
      announce,
      t,
      requestDelete,
      openEventMenu,
      itemId,
    ],
  );

  return (
    <section
      aria-label={t('views.agenda.title')}
      className="view view--agenda"
    >
      <header className="view__header">
        <h2>{t('views.agenda.title')}</h2>
        <span className="view__subtitle">
          {fmt.format(range.start, 'PPP')} – {fmt.format(range.end, 'PPP')}
        </span>
      </header>

      {showLoading && (
        <p className="view__loading" aria-hidden="true">
          {t('views.loading')}
        </p>
      )}

      <ul
        ref={listRef}
        role="listbox"
        tabIndex={0}
        aria-label={t('views.agenda.eventList')}
        aria-activedescendant={
          occurrences.length > 0 ? itemId(focusIndex) : undefined
        }
        onKeyDown={handleKeyDown}
        className="agenda-list"
      >
        {occurrences.length === 0 ? (
          <li role="presentation" className="agenda-list__empty">
            {t('views.agenda.empty')}
          </li>
        ) : (
          renderOccurrences(occurrences, focusIndex, {
            groupRows,
            calendarById,
            labelById,
            fmt,
            t,
            itemId,
            onSelect: setFocusIndex,
            onOpen: (event) => {
              openEventDialog(event);
            },
            onContextMenu: (event) => {
              void openEventMenu(event);
            },
          })
        )}
      </ul>

      <ConfirmDialog
        isOpen={confirmTarget !== null}
        onClose={() => setConfirmTarget(null)}
        onConfirm={() => {
          if (confirmTarget) void performDelete(confirmTarget, 'series');
        }}
        title={t('dialogs.confirm.deleteEventTitle')}
        message={t('dialogs.confirm.deleteEventMessage', {
          title: confirmTarget?.title ?? '',
        })}
      />

      <DeleteEventScopeDialog
        isOpen={scopeTarget !== null}
        onClose={() => setScopeTarget(null)}
        title={scopeTarget?.title ?? ''}
        event={scopeTarget}
        onOccurrence={(send) => {
          if (scopeTarget) void performDelete(scopeTarget, 'occurrence', send);
        }}
        onThisAndFuture={(send) => {
          if (scopeTarget)
            void performDelete(scopeTarget, 'this_and_future', send);
        }}
        onSeries={(send) => {
          if (scopeTarget) void performDelete(scopeTarget, 'series', send);
        }}
      />
    </section>
  );
}

interface RenderContext {
  /** The folded rows, keyed `<eventInstanceKey>@<dayKey>` — what each row
   *  stands for, when it stands for more than itself. */
  groupRows: Map<string, CollapsedRow<CalendarEvent>>;
  calendarById: Parameters<typeof resolveEventColor>[1];
  labelById: Parameters<typeof resolveEventColor>[2];
  fmt: ReturnType<typeof useDateFormat>;
  t: (key: string, vars?: Record<string, unknown>) => string;
  itemId: (i: number) => string;
  onSelect: (i: number) => void;
  onOpen: (event: CalendarEvent, index: number) => void;
  onContextMenu: (event: CalendarEvent, index: number) => void;
}

function renderOccurrences(
  occurrences: DayOccurrence[],
  focusIndex: number,
  ctx: RenderContext,
): JSX.Element[] {
  const out: JSX.Element[] = [];
  let lastDayKey: string | null = null;

  occurrences.forEach((occ, i) => {
    const { ev, day, span } = occ;
    const dayKey = localDateKey(day);

    // Visual-only day separator. aria-hidden so the screen reader
    // never reads it; the date is encoded into each option's aria-label.
    if (dayKey !== lastDayKey) {
      lastDayKey = dayKey;
      out.push(
        <li
          key={`sep-${dayKey}`}
          role="presentation"
          aria-hidden="true"
          className="agenda-list__day"
        >
          {ctx.fmt.format(day, 'PPPP')}
        </li>,
      );
    }

    const cal = ctx.calendarById.get(ev.calendar_id);
    const color = resolveEventColor(ev, ctx.calendarById, ctx.labelById);
    const timeLabel = ev.all_day
      ? ctx.t('views.allDay')
      : `${ctx.fmt.format(new Date(ev.start), 'p')} – ${ctx.fmt.format(new Date(ev.end), 'p')}`;
    const ariaBase = ctx.t('views.agenda.eventLabel', {
      day: ctx.fmt.format(day, 'PPPP'),
      title: ev.title,
      time: timeLabel,
      calendar: cal?.name ?? '—',
    });
    // What this row stands for, if it stands for more than itself. The count
    // comes from the group, so a copy in a switched-off calendar is counted
    // too — that is what makes it match what the user knows they keep.
    const groupRow = ctx.groupRows.get(`${eventInstanceKey(ev)}@${dayKey}`);
    const groupSuffix = groupRow?.group
      ? groupRow.diverged
        ? ctx.t('views.eventGroupDivergedSuffix', {
            count: groupRow.otherMembers,
          })
        : ctx.t('views.eventGroupSuffix', {
            count: groupRow.otherMembers,
            calendars: groupRow.calendarIds
              .map((id) => ctx.calendarById.get(id)?.name ?? id)
              .join(', '),
          })
      : '';
    const aria =
      (span
        ? ariaBase +
          ctx.t('views.multiDaySuffix', {
            day: span.dayIndex,
            total: span.totalDays,
          })
        : ariaBase) +
      groupSuffix +
      (ev.cancelled ? ctx.t('views.eventCancelledSuffix') : '');
    const focused = i === focusIndex;

    out.push(
      <li
        key={`${eventInstanceKey(ev)}@${dayKey}`}
        id={ctx.itemId(i)}
        role="option"
        aria-selected={focused}
        aria-label={aria}
        className={
          'agenda-list__item' +
          (focused ? ' agenda-list__item--focused' : '') +
          (ev.cancelled ? ' agenda-list__item--cancelled' : '') +
          (span ? ' agenda-list__item--multiday' : '')
        }
        style={
          color.hex
            ? ({ '--event-color': color.hex } as React.CSSProperties)
            : undefined
        }
        onClick={() => ctx.onSelect(i)}
        onDoubleClick={(e) => {
          // Open the editor, mirroring the task chips (single click just
          // moves focus; the keyboard path is Enter).
          e.stopPropagation();
          ctx.onSelect(i);
          ctx.onOpen(ev, i);
        }}
        onContextMenu={(e) => {
          e.preventDefault();
          e.stopPropagation();
          ctx.onSelect(i);
          ctx.onContextMenu(ev, i);
        }}
      >
        <span className="agenda-list__time">{timeLabel}</span>
        <span className="agenda-list__title">
          {ev.title}
          {span && (
            <span className="agenda-list__span">
              {' '}
              {ctx.t('views.multiDayCompact', {
                day: span.dayIndex,
                total: span.totalDays,
              })}
            </span>
          )}
        </span>
        {cal && <span className="agenda-list__cal">{cal.name}</span>}
      </li>,
    );
  });

  return out;
}
