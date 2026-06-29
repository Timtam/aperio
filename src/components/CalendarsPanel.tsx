import { useEffect, useId, useMemo, useState } from 'react';
import { FocusableNote } from '../a11y/FocusableNote';
import { useTranslation } from 'react-i18next';

import { useCalendarStore } from '../state/calendarStoreContext';
import { useTaskCascadeEnabled } from '../state/taskCascadeContext';
import type { CalendarDayViewMode } from '../state/TaskCascadeProvider';
import { useCalendarDefaultReminders } from '../state/useCalendarDefaultReminders';
import { RemindersEditor } from './RemindersEditor';
import { SoundPrefField } from './SoundPrefField';

/** Calendar day/week layout choices, rendered as a radio group. */
const DAY_VIEW_MODE_OPTIONS: readonly CalendarDayViewMode[] = ['grid', 'list'];

/**
 * Calendars settings panel — per-calendar default reminders + sounds.
 *
 * Lives in Settings → Kalender (see `SettingsDialog`'s `TAB_ORDER`). The
 * motivating use case is iOS Default Alert Times for CalDAV calendars: iOS
 * applies them locally at notification time and never writes a VALARM into
 * the VEVENT, so iCloud events without an explicit per-event alarm come
 * back from the wire with an empty `reminders` array and the user sees a
 * mismatch (iPhone alarm rings, Aperio shows nothing). The per-calendar
 * default closes the gap on the Aperio side. Local calendars work too.
 *
 * Accessibility — master/detail (DESIGN.md §6.5 keyboard-first idiom):
 * an earlier version rendered every calendar's full reminders editor +
 * sound picker inline, which produced ~8–12 focusable controls PER calendar
 * (dozens of accounts × calendars → 70–100+ tab stops) and, worse, never
 * told a screen-reader user WHICH calendar a focused control belonged to —
 * the calendar name was a bare `<span>`, the account an `<h3>`, neither
 * associated with the controls, so NVDA users needed object navigation.
 *
 * Now the panel is a single `role="listbox"` of calendars (one tab stop,
 * arrow-key navigable, grouped visually by account) where each option's
 * accessible name carries "{account} › {calendar}, {summary}" so the
 * source is always announced; plus a detail region that shows the editor
 * for ONLY the selected calendar, headed "Erinnerungen – {account} ›
 * {calendar}". Selection follows focus, so arrowing the list live-swaps the
 * detail; tabbing once lands in the selected calendar's editor and nothing
 * else. Tab count drops from ~per-calendar×N to "1 (list) + the one
 * calendar being edited".
 *
 * The panel reads the calendar catalog from `CalendarStore`; the hook fans
 * out one `getUserPref` per calendar on mount and writes are debounced.
 */
export function CalendarsPanel() {
  const { t } = useTranslation();
  const { calendars, accounts } = useCalendarStore();
  // Calendar day/week layout is a synced view preference (also toggled from the
  // calendar toolbar); both write the same `calendar.dayViewMode` pref.
  const { dayViewMode, setDayViewMode } = useTaskCascadeEnabled();

  const calendarIds = useMemo(() => calendars.map((c) => c.id), [calendars]);
  const { getDefaultsFor, setDefaultsFor, hydrating } =
    useCalendarDefaultReminders(calendarIds);

  // Group calendars by their owning account so the selector reads as
  // "iCloud > Privat | Arbeit" rather than a flat alphabetical list.
  const accountNameById = useMemo(() => {
    const map = new Map<string, string>();
    accounts.forEach((a) => map.set(a.id, a.display_name));
    return map;
  }, [accounts]);

  const groups = useMemo(() => {
    const byAccount = new Map<string, typeof calendars>();
    calendars.forEach((c) => {
      const list = byAccount.get(c.account_id) ?? [];
      list.push(c);
      byAccount.set(c.account_id, list);
    });
    return [...byAccount.entries()].map(([accountId, cals]) => ({
      accountId,
      accountName:
        accountNameById.get(accountId) ??
        (accountId === 'local'
          ? t('dialogs.settings.calendars.localAccount')
          : accountId),
      calendars: cals,
    }));
  }, [calendars, accountNameById, t]);

  // Flat id order for arrow-key navigation across the account groups.
  const orderedIds = useMemo(
    () => groups.flatMap((g) => g.calendars.map((c) => c.id)),
    [groups],
  );

  const [selectedId, setSelectedId] = useState<string | null>(null);

  // Keep a valid selection: default to the first calendar and recover if
  // the selected one disappears (account removed mid-session, etc.).
  useEffect(() => {
    if (orderedIds.length === 0) {
      if (selectedId !== null) setSelectedId(null);
      return;
    }
    if (selectedId === null || !orderedIds.includes(selectedId)) {
      setSelectedId(orderedIds[0]);
    }
  }, [orderedIds, selectedId]);

  const idPrefix = useId();
  const optionId = (id: string) => `${idPrefix}-opt-${id}`;
  const detailHeadingId = `${idPrefix}-detail-h`;

  const dayViewModeHeadingId = useId();
  const dayViewModeHintId = useId();
  const dayViewModeGroupId = useId();

  // Keep the active option visible: with aria-activedescendant the browser
  // doesn't move DOM focus, so it won't auto-scroll the selection into view
  // inside the (capped-height, scrollable) listbox. `block: 'nearest'`
  // minimises movement. Optional-chained so it's a no-op where
  // scrollIntoView isn't implemented (jsdom).
  useEffect(() => {
    if (!selectedId) return;
    const el = document.getElementById(`${idPrefix}-opt-${selectedId}`);
    el?.scrollIntoView?.({ block: 'nearest' });
  }, [selectedId, idPrefix]);

  // Account name + calendar for the currently selected option (detail head).
  const selected = useMemo(() => {
    for (const g of groups) {
      const cal = g.calendars.find((c) => c.id === selectedId);
      if (cal) return { cal, accountName: g.accountName };
    }
    return null;
  }, [groups, selectedId]);

  // Spoken summary of a calendar's default-reminder count — folded into the
  // option's accessible name so the screen reader announces the state
  // without the user having to read the detail pane. Singular/plural picked
  // in code to stay independent of the i18next plural-suffix convention.
  const summaryFor = (id: string): string => {
    const n = getDefaultsFor(id).length;
    if (n === 0) return t('dialogs.settings.calendars.reminderSummaryNone');
    if (n === 1) return t('dialogs.settings.calendars.reminderSummaryOne');
    return t('dialogs.settings.calendars.reminderSummaryOther', { count: n });
  };

  const selectAt = (index: number) => {
    if (orderedIds.length === 0) return;
    const clamped = Math.min(orderedIds.length - 1, Math.max(0, index));
    setSelectedId(orderedIds[clamped]);
  };

  const handleKey = (e: React.KeyboardEvent<HTMLUListElement>) => {
    if (e.ctrlKey || e.metaKey || e.altKey) return;
    const cur = selectedId ? orderedIds.indexOf(selectedId) : -1;
    switch (e.key) {
      case 'ArrowDown':
        e.preventDefault();
        selectAt(cur + 1);
        return;
      case 'ArrowUp':
        e.preventDefault();
        selectAt(cur - 1);
        return;
      case 'Home':
        e.preventDefault();
        selectAt(0);
        return;
      case 'End':
        e.preventDefault();
        selectAt(orderedIds.length - 1);
        return;
      default:
        return;
    }
  };

  return (
    <div className="settings-panel calendars-panel">
      <FocusableNote className="form__hint">
        {t('dialogs.settings.calendars.hint')}
      </FocusableNote>

      {/* Calendar day/week layout — a top-level view preference, so it sits at
          the top of the panel above the per-calendar defaults. Two options with
          a clear visual trade-off (hour-grid vs compact list); a radiogroup
          keeps them side-by-side with arrow-key nav. The toolbar exposes the
          same toggle as a quick-switch; both write the synced pref. */}
      <section
        className="calendars-panel__group"
        aria-label={t('dialogs.settings.calendars.dayViewMode.heading')}
      >
        <h3
          id={dayViewModeHeadingId}
          className="calendars-panel__account"
        >
          {t('dialogs.settings.calendars.dayViewMode.heading')}
        </h3>
        <p id={dayViewModeHintId} className="form__hint">
          {t('dialogs.settings.calendars.dayViewMode.hint')}
        </p>
        <div
          role="radiogroup"
          id={dayViewModeGroupId}
          aria-labelledby={dayViewModeHeadingId}
          aria-describedby={dayViewModeHintId}
          className="tasks-settings__radiogroup"
        >
          {DAY_VIEW_MODE_OPTIONS.map((option) => (
            <label key={option} className="tasks-settings__radio">
              <input
                type="radio"
                name={dayViewModeGroupId}
                value={option}
                checked={dayViewMode === option}
                onChange={() => setDayViewMode(option)}
              />
              <span>
                {t(`dialogs.settings.calendars.dayViewMode.options.${option}`)}
              </span>
            </label>
          ))}
        </div>
      </section>

      {/* §14.4 global notification-sound default. Sits above the
          per-calendar selector because it's the fallback every calendar /
          event inherits when nothing more specific is set. */}
      <section
        className="calendars-panel__group"
        aria-label={t('dialogs.settings.notifications.heading')}
      >
        <h3 className="calendars-panel__account">
          {t('dialogs.settings.notifications.heading')}
        </h3>
        <p className="form__hint">
          {t('dialogs.settings.notifications.hint')}
        </p>
        <SoundPrefField
          prefKey="sound.global"
          allowInherit={false}
          legend={t('dialogs.settings.notifications.globalLabel')}
        />
      </section>

      {hydrating && (
        <p className="form__hint" aria-live="polite">
          {t('views.loading')}
        </p>
      )}

      {!hydrating && calendars.length === 0 && (
        <FocusableNote className="form__hint">
          {t('dialogs.settings.calendars.empty')}
        </FocusableNote>
      )}

      {!hydrating && calendars.length > 0 && (
        <div className="calendars-panel__master-detail">
          {/* Master: one keyboard-navigable list of calendars. The visual
              account sub-headers are presentational (each option's own
              accessible name already carries the account), so arrow keys
              walk the options uninterrupted. */}
          <ul
            role="listbox"
            tabIndex={0}
            aria-label={t('dialogs.settings.calendars.selectorLabel')}
            aria-activedescendant={
              selectedId ? optionId(selectedId) : undefined
            }
            onKeyDown={handleKey}
            className="calendars-panel__selector"
          >
            {groups.map((group) => (
              <li
                key={group.accountId}
                role="presentation"
                className="calendars-panel__selector-group"
              >
                <span
                  className="calendars-panel__account"
                  aria-hidden="true"
                >
                  {group.accountName}
                </span>
                <ul
                  role="presentation"
                  className="calendars-panel__selector-sublist"
                >
                  {group.calendars.map((cal) => {
                    const isSel = cal.id === selectedId;
                    return (
                      <li
                        key={cal.id}
                        id={optionId(cal.id)}
                        role="option"
                        aria-selected={isSel}
                        aria-label={t(
                          'dialogs.settings.calendars.optionLabel',
                          {
                            account: group.accountName,
                            calendar: cal.name,
                            summary: summaryFor(cal.id),
                          },
                        )}
                        className={
                          'calendars-panel__option' +
                          (isSel ? ' calendars-panel__option--selected' : '')
                        }
                        onClick={() => setSelectedId(cal.id)}
                      >
                        <span
                          className="calendars-panel__swatch"
                          aria-hidden="true"
                          style={
                            cal.color?.hex
                              ? { background: cal.color.hex }
                              : undefined
                          }
                        />
                        <span className="calendars-panel__name">
                          {cal.name}
                        </span>
                        <span
                          className="calendars-panel__option-summary"
                          aria-hidden="true"
                        >
                          {getDefaultsFor(cal.id).length || ''}
                        </span>
                      </li>
                    );
                  })}
                </ul>
              </li>
            ))}
          </ul>

          {/* Detail: editor for the selected calendar only. The region is
              named after the calendar so focus entering it (Tab from the
              list) announces "Erinnerungen – {account} › {calendar}". */}
          {selected && (
            <section
              className="calendars-panel__detail"
              aria-labelledby={detailHeadingId}
            >
              <h3
                id={detailHeadingId}
                className="calendars-panel__detail-heading"
              >
                {t('dialogs.settings.calendars.detailHeading', {
                  account: selected.accountName,
                  calendar: selected.cal.name,
                })}
              </h3>
              {/* `key` resets the editor's internal state when the selected
                  calendar changes, so the controls always reflect the new
                  calendar rather than carrying over the previous one's. */}
              <RemindersEditor
                key={selected.cal.id}
                value={getDefaultsFor(selected.cal.id)}
                onChange={(next) => setDefaultsFor(selected.cal.id, next)}
                mode="event"
              />
              {/* §14.4 per-calendar default sound — inherits the global
                  default unless overridden. */}
              <SoundPrefField
                key={`sound-${selected.cal.id}`}
                prefKey={`sound.calendar.${selected.cal.id}`}
              />
            </section>
          )}
        </div>
      )}
    </div>
  );
}
