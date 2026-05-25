import { useMemo } from 'react';
import { FocusableNote } from '../a11y/FocusableNote';
import { useTranslation } from 'react-i18next';

import { useCalendarStore } from '../state/CalendarStore';
import { useCalendarDefaultReminders } from '../state/useCalendarDefaultReminders';
import { RemindersEditor } from './RemindersEditor';

/**
 * Calendars settings panel.
 *
 * Per-calendar default reminders. Lives in Settings → Kalender (see
 * `SettingsDialog`'s `TAB_ORDER`). The motivating use case is iOS
 * Default Alert Times for CalDAV calendars: iOS applies them locally
 * at notification time and never writes a VALARM into the VEVENT, so
 * iCloud events without an explicit per-event alarm come back from
 * the wire with an empty `reminders` array and the user sees a
 * mismatch between the iPhone (alarm rings) and Aperio (no alarm in
 * sight). The per-calendar default closes the gap on the Aperio side.
 *
 * Local calendars work too — a user who wants every "Birthdays" entry
 * to ping them at 09:00 the morning of can set a single absolute
 * default here instead of repeating the reminder on every row.
 *
 * Layout: one accordion-less row per calendar, grouped by account.
 * Each row hosts a `RemindersEditor` bound to the calendar's default
 * list. Removing the last reminder clears the per-calendar override.
 *
 * The panel reads the calendar catalog from `CalendarStore`; the
 * hook fans out one `getUserPref` per calendar on mount and writes
 * are debounced 150 ms so the inline minute input doesn't hammer
 * the wire.
 */
export function CalendarsPanel() {
  const { t } = useTranslation();
  const { calendars, accounts } = useCalendarStore();

  const calendarIds = useMemo(
    () => calendars.map((c) => c.id),
    [calendars],
  );
  const { getDefaultsFor, setDefaultsFor, hydrating } =
    useCalendarDefaultReminders(calendarIds);

  // Group calendars by their owning account so the panel reads as
  // "iCloud > Privat | Arbeit" rather than a flat alphabetical list.
  // Local-only setups still get the implicit "local" account header
  // — keeps the rendering pure even on a fresh install.
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

  return (
    <div className="settings-panel calendars-panel">
      <FocusableNote className="form__hint">
        {t('dialogs.settings.calendars.hint')}
      </FocusableNote>

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

      {!hydrating &&
        groups.map((group) => (
          <section
            key={group.accountId}
            className="calendars-panel__group"
            aria-label={t('dialogs.settings.calendars.accountHeading', {
              account: group.accountName,
            })}
          >
            <h3 className="calendars-panel__account">{group.accountName}</h3>
            <ul className="calendars-panel__list">
              {group.calendars.map((cal) => (
                <li key={cal.id} className="calendars-panel__row">
                  <header className="calendars-panel__row-header">
                    <span
                      className="calendars-panel__swatch"
                      aria-hidden="true"
                      style={
                        cal.color?.hex
                          ? { background: cal.color.hex }
                          : undefined
                      }
                    />
                    <span className="calendars-panel__name">{cal.name}</span>
                  </header>
                  {/* The reminders editor is "event" mode because the
                      defaults apply to calendar events. Task lists
                      have their own per-list defaults flow (not built
                      yet — task list defaults can ride on the same
                      hook later by passing the list id under the same
                      `calendar.{id}.defaultReminders` key, since list
                      ids are namespaced separately from calendar
                      ids). */}
                  <RemindersEditor
                    value={getDefaultsFor(cal.id)}
                    onChange={(next) => setDefaultsFor(cal.id, next)}
                    mode="event"
                  />
                </li>
              ))}
            </ul>
          </section>
        ))}
    </div>
  );
}
