import { useCallback, useEffect, useId, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';

import type { CalendarEvent } from '../../api/types';
import { useAutoFocus } from '../../hooks/useAutoFocus';
import { localDateKey } from '../../intl/dateKey';
import { useDateFormat } from '../../intl/dateFormat';
import { labelsLookup, resolveEventColor } from '../../intl/eventColor';
import { useCalendarStore } from '../../state/CalendarStore';
import { useDialogState } from '../../state/DialogState';
import { useEvents } from '../../state/useEvents';
import { useViewState } from '../../state/ViewState';
import { visibleRange } from '../../state/viewMath';

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
  const { anchor } = useViewState();
  const fmt = useDateFormat();
  const { openEventDialog } = useDialogState();

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

  const idPrefix = useId();
  const itemId = (i: number) => `${idPrefix}-item-${i}`;
  const listRef = useAutoFocus<HTMLUListElement>(!loading);

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.ctrlKey || e.metaKey || e.altKey) return;
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
        default:
          return;
      }
    },
    [events, focusIndex, openEventDialog],
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
