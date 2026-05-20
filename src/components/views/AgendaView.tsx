import { useCallback, useEffect, useId, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';

import type { CalendarEvent } from '../../api/types';
import { useAnnouncer } from '../../a11y/Announcer';
import { useAutoFocus } from '../../hooks/useAutoFocus';
import { localDateKey } from '../../intl/dateKey';
import { useDateFormat } from '../../intl/dateFormat';
import { labelsLookup, resolveEventColor } from '../../intl/eventColor';
import { useCalendarStore } from '../../state/CalendarStore';
import { useDialogState } from '../../state/DialogState';
import { useEvents } from '../../state/useEvents';
import { useViewState } from '../../state/ViewState';
import { visibleRange } from '../../state/viewMath';
import { duplicateEvent } from '../MoveCopyDialog';
import { ConfirmDialog } from '../ConfirmDialog';
import { DeleteEventScopeDialog } from '../DeleteEventScopeDialog';
import {
  addEventExdate,
  deleteEventById,
  isCommandError,
} from '../../api/client';

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
  const { colorLabels } = useCalendarStore();
  const labelById = useMemo(() => labelsLookup(colorLabels), [colorLabels]);

  const [focusIndex, setFocusIndex] = useState(0);

  useEffect(() => {
    if (focusIndex >= events.length) {
      setFocusIndex(Math.max(0, events.length - 1));
    }
  }, [events.length, focusIndex]);

  // Announce "Loading …" once on mount if we're still fetching. See
  // DayView for the rationale (mount-only, never on refetches).
  useEffect(() => {
    if (loading) announce(t('views.loading'));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const idPrefix = useId();
  const itemId = (i: number) => `${idPrefix}-item-${i}`;
  const listRef = useAutoFocus<HTMLUListElement>(!loading);

  const [confirmTarget, setConfirmTarget] = useState<CalendarEvent | null>(
    null,
  );
  const [scopeTarget, setScopeTarget] = useState<CalendarEvent | null>(null);

  const performDelete = useCallback(
    async (ev: CalendarEvent, scope: 'occurrence' | 'series') => {
      try {
        if (scope === 'occurrence' && ev.id.includes('@')) {
          const [seriesId, occIso] = ev.id.split('@');
          await addEventExdate(seriesId, occIso, ev.calendar_id);
          announce(
            t('dialogs.event.occurrenceDeleted', { title: ev.title }),
          );
        } else {
          const id = ev.id.includes('@') ? ev.id.split('@')[0] : ev.id;
          await deleteEventById(id, ev.calendar_id);
          announce(t('dialogs.event.deleted', { title: ev.title }));
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
    if (ev.id.includes('@') || ev.recurrence) {
      setScopeTarget(ev);
    } else {
      setConfirmTarget(ev);
    }
  }, []);

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.ctrlKey || e.metaKey) {
        if (e.key.toLowerCase() === 'd' && !e.shiftKey && !e.altKey) {
          e.preventDefault();
          const ev = events[focusIndex];
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
        const ev = events[focusIndex];
        if (ev) openMoveCopy({ kind: 'event', event: ev });
        return;
      }
      if (events.length === 0) return;
      switch (e.key) {
        case 'ArrowDown':
          e.preventDefault();
          setFocusIndex((i) => Math.min(i + 1, events.length - 1));
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
          setFocusIndex(events.length - 1);
          return;
        case 'Enter':
        case ' ':
        case 'Spacebar': {
          e.preventDefault();
          const ev = events[focusIndex];
          if (ev) openEventDialog(ev);
          return;
        }
        case 'Delete':
        case 'Backspace': {
          e.preventDefault();
          const ev = events[focusIndex];
          if (ev) requestDelete(ev);
          return;
        }
        default:
          return;
      }
    },
    [events, focusIndex, openEventDialog, openMoveCopy, announce, t, requestDelete],
  );

  return (
    <section
      aria-label={t('views.agenda.title')}
      className="view view--agenda"
    >
      <header className="view__header">
        <h2>{t('views.agenda.title')}</h2>
        <span className="view__subtitle">
          {fmt.format(range.start, 'PP')} – {fmt.format(range.end, 'PP')}
        </span>
      </header>

      {loading && (
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
          events.length > 0 ? itemId(focusIndex) : undefined
        }
        onKeyDown={handleKeyDown}
        className="agenda-list"
      >
        {events.length === 0 ? (
          <li role="presentation" className="agenda-list__empty">
            {t('views.agenda.empty')}
          </li>
        ) : (
          renderEvents(events, focusIndex, {
            calendarById,
            labelById,
            fmt,
            t,
            itemId,
            onSelect: setFocusIndex,
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
        onOccurrence={() => {
          if (scopeTarget) void performDelete(scopeTarget, 'occurrence');
        }}
        onSeries={() => {
          if (scopeTarget) void performDelete(scopeTarget, 'series');
        }}
      />
    </section>
  );
}

interface RenderContext {
  calendarById: Parameters<typeof resolveEventColor>[1];
  labelById: Parameters<typeof resolveEventColor>[2];
  fmt: ReturnType<typeof useDateFormat>;
  t: (key: string, vars?: Record<string, unknown>) => string;
  itemId: (i: number) => string;
  onSelect: (i: number) => void;
}

function renderEvents(
  events: CalendarEvent[],
  focusIndex: number,
  ctx: RenderContext,
): JSX.Element[] {
  const out: JSX.Element[] = [];
  let lastDayKey: string | null = null;

  events.forEach((ev, i) => {
    const start = new Date(ev.start);
    const dayKey = localDateKey(start);

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
          {ctx.fmt.format(start, 'PPPP')}
        </li>,
      );
    }

    const cal = ctx.calendarById.get(ev.calendar_id);
    const color = resolveEventColor(ev, ctx.calendarById, ctx.labelById);
    const timeLabel = ev.all_day
      ? ctx.t('views.allDay')
      : `${ctx.fmt.format(start, 'p')} – ${ctx.fmt.format(new Date(ev.end), 'p')}`;
    const aria = color.labelName
      ? ctx.t('views.agenda.eventLabelWithLabel', {
          day: ctx.fmt.format(start, 'PPPP'),
          title: ev.title,
          time: timeLabel,
          calendar: cal?.name ?? '—',
          label: color.labelName,
        })
      : ctx.t('views.agenda.eventLabel', {
          day: ctx.fmt.format(start, 'PPPP'),
          title: ev.title,
          time: timeLabel,
          calendar: cal?.name ?? '—',
        });
    const focused = i === focusIndex;

    out.push(
      <li
        key={ev.id}
        id={ctx.itemId(i)}
        role="option"
        aria-selected={focused}
        aria-label={aria}
        className={
          'agenda-list__item' + (focused ? ' agenda-list__item--focused' : '')
        }
        style={
          color.hex
            ? ({ '--event-color': color.hex } as React.CSSProperties)
            : undefined
        }
        onClick={() => ctx.onSelect(i)}
      >
        <span className="agenda-list__time">{timeLabel}</span>
        <span className="agenda-list__title">{ev.title}</span>
        {cal && <span className="agenda-list__cal">{cal.name}</span>}
      </li>,
    );
  });

  return out;
}
