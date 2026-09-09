import { useEffect } from 'react';
import { useTranslation } from 'react-i18next';

import { setReminderLabels } from '../api/client';

/**
 * Keep the reminder notifications' wording in sync with the app language.
 *
 * The host fires reminders from Rust — on a schedule, with the window closed
 * if need be — and has no i18n. That is the same situation the tray menu is in
 * (`useTrayMenuLabels`), with one difference that made it worth fixing rather
 * than living with: the tray showed an English placeholder, while a reminder
 * showed something WRONG. Every body was formatted `%H:%M`, and an all-day
 * event starts at local midnight, so a birthday reminder announced "00:00".
 * Mobile has said "Ganztägig" since it shipped, and its `notificationBody`
 * names the desktop's bug in its own doc comment.
 *
 * Month names come from `Intl` rather than the translation files — the same
 * source mobile's scheduler uses, so the two surfaces spell June the same way
 * — and `dayMonth` carries the ORDER, because that is not the same in every
 * language: "24. Juni" against "June 24".
 */
export function useReminderLabels(): void {
  const { t, i18n } = useTranslation();
  useEffect(() => {
    const month = new Intl.DateTimeFormat(i18n.language, { month: 'long' });
    // Any year works; only the month index is read. 2026 rather than "now" —
    // this must not depend on when it runs.
    const months = Array.from({ length: 12 }, (_, m) =>
      month.format(new Date(2026, m, 1)),
    );
    void setReminderLabels({
      allDay: t('dialogs.reminders.allDay'),
      allDayRange: t('dialogs.reminders.allDayRange'),
      dayMonth: t('dialogs.reminders.dayMonth'),
      months,
    }).catch(() => {
      // Not running under Tauri — there is no scheduler to tell.
    });
  }, [t, i18n.language]);
}
