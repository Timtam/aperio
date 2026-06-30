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
          getItemId={(c) => c.id}
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
            <>
              {/* `key` resets the editor's internal state when the selected
                  calendar changes, so the controls always reflect the new
                  calendar rather than carrying over the previous one's. */}
              <RemindersEditor
                key={cal.id}
                value={getDefaultsFor(cal.id)}
                onChange={(next) => setDefaultsFor(cal.id, next)}
                mode="event"
              />
              {/* §14.4 per-calendar default sound — inherits the global
                  default unless overridden. */}
              <SoundPrefField
                key={`sound-${cal.id}`}
                prefKey={`sound.calendar.${cal.id}`}
              />
            </>
          )}
        />
      )}
    </div>
  );
}
