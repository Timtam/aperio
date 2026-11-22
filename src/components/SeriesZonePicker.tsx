import {
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
} from 'react';
import { useTranslation } from 'react-i18next';

import {
  delayedSay,
  listedZoneLabels,
  unusableZones,
  zoneGroups,
  zoneOffsets,
  zonePickerList,
  zoneRegionNames,
  type ZonePick,
} from '@aperio/shared';

import { useAnnouncer } from '../a11y/announcerContext';
import { Modal } from './Modal';

/** How long the typing must pause before the count line is said (122b). */
const COUNT_PAUSE_MS = 600;
/** How far Page Up and Page Down move the marking. */
const PAGE = 10;

/** The zones the series' calendar stores, in the manifest's form (97a). */
export type CalendarZones = 'all' | 'none' | string[];

export interface SeriesZonePickerProps {
  isOpen: boolean;
  /** The series' start, RFC 3339: a city shows its offset then. */
  startAt: string;
  /** Today, RFC 3339, from this surface's clock: the groups are read from it. */
  today: string;
  /** The zone the series stores, as stored; `null` for none. */
  stored: string | null;
  /** The device's zone name as the runtime reports it (`deviceZoneName`). */
  deviceZone: string | null;
  calendarZones: CalendarZones;
  /** A row was chosen; the caller closes the picker and applies it. */
  onPick: (pick: ZonePick) => void;
  /** Escape, the close button or the backdrop: nothing was chosen. */
  onCancel: () => void;
}

/**
 * The series time zone picker on the desktop (DESIGN-series-time-zone.md,
 * stage 11, "Die Zonenliste"): a dialog nested in the editor, its search field
 * with the first focus and the count line as its description, and a list in
 * which the arrows move a marking (the combobox pattern of the attendee
 * picker). Page Up and Page Down move ten rows, Home and End to the first and
 * last (131a). Enter takes the marked row and does nothing without one, or on
 * a marked-off row, whose text already says why. A marking alone changes
 * nothing. Escape closes the picker alone; the Modal puts the focus back on
 * the button that opened it.
 *
 * The rows are the shared `zonePickerList`: without a search the zone in
 * effect, the device's zone, UTC, then the 62 groups (118a); with a search the
 * cities, or the groups for offsets alone (121a). The count line is said once
 * the typing pauses (122b), and a choice or a close cancels it (measured
 * 2026-11-22).
 */
export function SeriesZonePicker({
  isOpen,
  startAt,
  today,
  stored,
  deviceZone,
  calendarZones,
  onPick,
  onCancel,
}: SeriesZonePickerProps) {
  const { t, i18n } = useTranslation();
  const announce = useAnnouncer();
  const id = useId();
  const inputRef = useRef<HTMLInputElement>(null);
  const typed = useRef(false);
  const [query, setQuery] = useState('');
  const [active, setActive] = useState(-1);

  const sayCount = useMemo(() => delayedSay((text) => announce(text), COUNT_PAUSE_MS), [announce]);
  useEffect(() => () => sayCount.cancel(), [sayCount]);
  useEffect(() => {
    setQuery('');
    setActive(-1);
    typed.current = false;
    if (!isOpen) sayCount.cancel();
  }, [isOpen, sayCount]);

  const calendarKey = JSON.stringify(calendarZones);
  const unusable = useMemo(
    () => unusableZones(listedZoneLabels().map((label) => label.zone), stored),
    [stored],
  );
  const groups = useMemo(
    () =>
      isOpen
        ? zoneGroups({
            today,
            stored,
            device: deviceZone,
            calendar_zones: JSON.parse(calendarKey) as CalendarZones,
            unusable,
          })
        : null,
    [isOpen, today, stored, deviceZone, calendarKey, unusable],
  );
  const offsets = useMemo(() => (isOpen ? zoneOffsets(startAt, today) : null), [isOpen, startAt, today]);
  const list = useMemo(() => {
    if (groups === null || offsets === null) return null;
    const language = i18n.language;
    const formatDay = (day: string) =>
      new Intl.DateTimeFormat(language, {
        day: 'numeric',
        month: 'long',
        year: 'numeric',
        timeZone: 'UTC',
      }).format(new Date(`${day}T00:00:00Z`));
    return zonePickerList({
      query,
      groups,
      offsets,
      regionNames: zoneRegionNames(t),
      stored,
      device: deviceZone,
      t,
      formatDay,
    });
  }, [groups, offsets, query, stored, deviceZone, t, i18n.language]);

  // The count line, once the typing pauses; never for the list as it opens,
  // where it is the search field's description.
  useEffect(() => {
    if (list !== null && typed.current) {
      typed.current = false;
      sayCount.schedule(list.count);
    }
  }, [list, sayCount]);

  const optionId = (index: number) => `${id}-option-${index}`;
  useEffect(() => {
    if (active < 0) return;
    const option = document.getElementById(optionId(active));
    if (option && typeof option.scrollIntoView === 'function') {
      option.scrollIntoView({ block: 'nearest' });
    }
    // optionId only joins `id`, which never changes.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [active]);

  const rows = list?.rows ?? [];
  const choose = (index: number) => {
    const row = rows[index];
    if (!row || !row.selectable) return;
    sayCount.cancel();
    onPick(row.pick);
  };

  const onKeyDown = (event: ReactKeyboardEvent<HTMLInputElement>) => {
    const last = rows.length - 1;
    const markAt = (index: number) => {
      event.preventDefault();
      if (last < 0) return;
      setActive(Math.max(0, Math.min(last, index)));
    };
    switch (event.key) {
      case 'ArrowDown':
        markAt(active < 0 ? 0 : active + 1);
        return;
      case 'ArrowUp':
        markAt(active < 0 ? 0 : active - 1);
        return;
      case 'PageDown':
        markAt(active + PAGE);
        return;
      case 'PageUp':
        markAt(active < 0 ? 0 : active - PAGE);
        return;
      case 'Home':
        markAt(0);
        return;
      case 'End':
        markAt(last);
        return;
      case 'Enter':
        // Never the surrounding form's submit.
        event.preventDefault();
        if (active >= 0) choose(active);
        return;
      default:
        return;
    }
  };

  const listId = `${id}-list`;
  const countId = `${id}-count`;
  return (
    <Modal
      isOpen={isOpen}
      onClose={() => {
        sayCount.cancel();
        onCancel();
      }}
      title={t('dialogs.seriesTimeZone.title')}
      initialFocusRef={inputRef}
      className="zone-picker"
    >
      <label className="form__field">
        <span className="form__label">{t('dialogs.seriesTimeZone.searchLabel')}</span>
        <input
          ref={inputRef}
          type="text"
          role="combobox"
          aria-autocomplete="list"
          aria-expanded={true}
          aria-controls={listId}
          aria-activedescendant={active >= 0 && active < rows.length ? optionId(active) : undefined}
          aria-describedby={countId}
          autoComplete="off"
          spellCheck={false}
          value={query}
          onChange={(event) => {
            typed.current = true;
            setQuery(event.target.value);
            setActive(-1);
          }}
          onKeyDown={onKeyDown}
        />
      </label>
      <p id={countId} className="zone-picker__count">
        {list?.count ?? ''}
      </p>
      <ul id={listId} role="listbox" aria-label={t('dialogs.seriesTimeZone.listLabel')} className="zone-picker__list">
        {rows.map((row, index) => (
          <li
            key={row.key}
            id={optionId(index)}
            role="option"
            aria-selected={index === active}
            aria-disabled={row.selectable ? undefined : true}
            // Mouse-down, so the choice lands before the search field's
            // blur; the main button only, never a right or middle click.
            onMouseDown={(event) => {
              if (event.button !== 0) return;
              event.preventDefault();
              setActive(index);
              choose(index);
            }}
            className={
              'zone-picker__option' +
              (index === active ? ' zone-picker__option--marked' : '') +
              (row.selectable ? '' : ' zone-picker__option--refused')
            }
          >
            {row.text}
          </li>
        ))}
      </ul>
    </Modal>
  );
}
