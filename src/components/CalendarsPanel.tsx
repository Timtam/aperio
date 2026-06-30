import { useId, useMemo } from 'react';
import { FocusableNote } from '../a11y/FocusableNote';
import { useTranslation } from 'react-i18next';

import { useCalendarStore } from '../state/calendarStoreContext';
import { useTaskCascadeEnabled } from '../state/taskCascadeContext';
import type { CalendarDayViewMode } from '../state/TaskCascadeProvider';
import { useCalendarDefaultReminders } from '../state/useCalendarDefaultReminders';
import { RemindersEditor } from './RemindersEditor';
import { SettingsSelectorDetail } from './SettingsSelectorDetail';
import { SoundPrefField } from './SoundPrefField';

/** Calendar day/week layout choices, rendered as a radio group. */
const DAY_VIEW_MODE_OPTIONS: readonly CalendarDayViewMode[] = ['grid', 'list'];

/** Format a minutes-from-midnight value as a zero-padded "HH:MM" wall clock.
 *  1440 reads as "24:00" (the end-of-day sentinel). */
function formatHhMm(minutes: number): string {
  const hh = Math.floor(minutes / 60);
  const mm = minutes % 60;
  return `${String(hh).padStart(2, '0')}:${String(mm).padStart(2, '0')}`;
}

/** Half-hour START options: 00:00 … 23:30 (minutes 0, 30, … 1410). The window
 *  start can't be the end-of-day sentinel, so 24:00 is excluded. */
const DAY_START_OPTIONS: readonly number[] = Array.from(
  { length: 48 },
  (_, i) => i * 30,
);

/** Half-hour END options: 00:30 … 24:00 (minutes 30, 60, … 1440). The window
 *  end can't be midnight, so 00:00 is excluded; 1440 is the "24:00" sentinel. */
const DAY_END_OPTIONS: readonly number[] = Array.from(
  { length: 48 },
  (_, i) => (i + 1) * 30,
);

/** Stable id accessor for the selector (module scope so its identity holds). */
const calendarItemId = (c: { id: string }) => c.id;

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
 * Accessibility — master/detail (DESIGN.md §6.5 keyboard-first idiom),
 * factored into the shared `SettingsSelectorDetail`: an earlier version
 * rendered every calendar's full reminders editor + sound picker inline,
 * which produced ~8–12 focusable controls PER calendar (dozens of accounts ×
 * calendars → 70–100+ tab stops) and, worse, never told a screen-reader user
 * WHICH calendar a focused control belonged to. Now the panel is a single
 * `role="listbox"` of calendars (one tab stop, arrow-key navigable, grouped
 * visually by account) where each option's accessible name carries
 * "{account} › {calendar}, {summary}", plus a detail region that shows the
 * editor for ONLY the selected calendar.
 *
 * The panel reads the calendar catalog from `CalendarStore`; the hook fans
 * out one `getUserPref` per calendar on mount and writes are debounced.
 */
export function CalendarsPanel() {
  const { t } = useTranslation();
  const { calendars, accounts } = useCalendarStore();
  // Calendar day/week layout is a synced view preference (also toggled from the
  // calendar toolbar); both write the same `calendar.dayViewMode` pref.
  const { dayViewMode, setDayViewMode, dayStartMin, dayEndMin, setDayWindow } =
    useTaskCascadeEnabled();

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

  const selectorGroups = useMemo(() => {
    const byAccount = new Map<string, typeof calendars>();
    calendars.forEach((c) => {
      const list = byAccount.get(c.account_id) ?? [];
      list.push(c);
      byAccount.set(c.account_id, list);
    });
    return [...byAccount.entries()].map(([accountId, cals]) => ({
      id: accountId,
      label:
        accountNameById.get(accountId) ??
        (accountId === 'local'
          ? t('dialogs.settings.calendars.localAccount')
          : accountId),
      items: cals,
    }));
  }, [calendars, accountNameById, t]);

  const dayViewModeHeadingId = useId();
  const dayViewModeHintId = useId();
  const dayViewModeGroupId = useId();
  const dayWindowHeadingId = useId();
  const dayWindowHintId = useId();

  // Spoken summary of a calendar's default-reminder count — folded into the
  // option's accessible name so the screen reader announces the state without
  // the user having to read the detail pane. Singular/plural picked in code to
  // stay independent of the i18next plural-suffix convention.
  const summaryFor = (id: string): string => {
    const n = getDefaultsFor(id).length;
    if (n === 0) return t('dialogs.settings.calendars.reminderSummaryNone');
    if (n === 1) return t('dialogs.settings.calendars.reminderSummaryOne');
    return t('dialogs.settings.calendars.reminderSummaryOther', { count: n });
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
        <h3 id={dayViewModeHeadingId} className="calendars-panel__account">
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

      {/* Visible day window — the START + END of the calendar hour-grid. Two
          native <select>s of half-hour times (NOT a slider) so the primary
          (blind) user can operate it with a screen reader + keyboard. Each
          change validates the pair via setDayWindow (snap to half-hour, clamp,
          full-day fallback when start >= end). Synced like dayViewMode. */}
      <section
        className="calendars-panel__group"
        aria-label={t('dialogs.settings.calendars.dayWindow.heading')}
      >
        <h3 id={dayWindowHeadingId} className="calendars-panel__account">
          {t('dialogs.settings.calendars.dayWindow.heading')}
        </h3>
        <p id={dayWindowHintId} className="form__hint">
          {t('dialogs.settings.calendars.dayWindow.hint')}
        </p>
        <label className="form__field">
          <span className="form__label">
            {t('dialogs.settings.calendars.dayWindow.startLabel')}
          </span>
          <select
            value={dayStartMin}
            aria-describedby={dayWindowHintId}
            onChange={(e) => setDayWindow(Number(e.target.value), dayEndMin)}
          >
            {DAY_START_OPTIONS.map((min) => (
              <option key={min} value={min}>
                {formatHhMm(min)}
              </option>
            ))}
          </select>
        </label>
        <label className="form__field">
          <span className="form__label">
            {t('dialogs.settings.calendars.dayWindow.endLabel')}
          </span>
          <select
            value={dayEndMin}
            aria-describedby={dayWindowHintId}
            onChange={(e) => setDayWindow(dayStartMin, Number(e.target.value))}
          >
            {DAY_END_OPTIONS.map((min) => (
              <option key={min} value={min}>
                {formatHhMm(min)}
              </option>
            ))}
          </select>
        </label>
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
        <p className="form__hint">{t('dialogs.settings.notifications.hint')}</p>
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
        <SettingsSelectorDetail
          groups={selectorGroups}
          getItemId={calendarItemId}
          getItemName={(c) => c.name}
          getItemSummary={(c) => summaryFor(c.id)}
          getItemBadge={(c) => getDefaultsFor(c.id).length || ''}
          getItemSwatchHex={(c) => c.color?.hex}
          withSwatch
          selectorLabel={t('dialogs.settings.calendars.selectorLabel')}
          optionLabel={({ account, name, summary }) =>
            t('dialogs.settings.calendars.optionLabel', {
              account,
              calendar: name,
              summary,
            })
          }
          detailHeading={({ account, name }) =>
            t('dialogs.settings.calendars.detailHeading', {
              account,
              calendar: name,
            })
          }
          renderDetail={(cal) => (
            // The selector re-keys this subtree per selection, so the editor
            // already resets to the newly-selected calendar's values.
            <>
              <RemindersEditor
                value={getDefaultsFor(cal.id)}
                onChange={(next) => setDefaultsFor(cal.id, next)}
                mode="event"
              />
              {/* §14.4 per-calendar default sound — inherits the global
                  default unless overridden. */}
              <SoundPrefField prefKey={`sound.calendar.${cal.id}`} />
            </>
          )}
        />
      )}
    </div>
  );
}
