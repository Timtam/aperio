// App-wide long-form, locale-aware date formatting for date VALUES shown as
// text (timestamps like "last synced", due/completed dates, reminder times, …).
// Long form = the month spelled out ("19. Juni 2026" / "June 19, 2026") rather
// than the cryptic numeric short form ("19.6.2026"). Always pass the app locale
// (i18n.language) so it follows the user's chosen/detected language.

/** Long-form date, no time: "19. Juni 2026" / "June 19, 2026". */
export function formatLongDate(date: Date, locale: string): string {
  return new Intl.DateTimeFormat(locale, { dateStyle: 'long' }).format(date);
}

/** Long-form date + short time: "19. Juni 2026 um 14:30" / "June 19, 2026 at
 *  2:30 PM" — for timestamps. */
export function formatLongDateTime(date: Date, locale: string): string {
  return new Intl.DateTimeFormat(locale, { dateStyle: 'long', timeStyle: 'short' }).format(date);
}
