/**
 * A temporary trace of the calendar an accepted quick-add offer hands to the
 * editor, into the log (`console.info` reaches the log file as
 * `aperio::webview` in every build). Toni, 2026-10-31: in release builds the
 * quick-add shows the offer's calendar, and the editor it opens sits on the
 * first one — while every test of the hand-off, with and without StrictMode,
 * passes. Each step that decides or changes the calendar writes one line.
 *
 * A calendar is written as its position in the catalog (`#2`, `-` for none,
 * `?` for one the catalog does not hold), never by its id: a Google calendar's
 * id is an e-mail address. No title reaches the log. Remove once the cause is
 * known (TODO).
 */

/** Where `id` sits in `calendars`, as the trace writes it. */
export function calendarPlace(
  calendars: readonly { id: string }[],
  id: string | null | undefined,
): string {
  if (!id) return '-';
  const at = calendars.findIndex((c) => c.id === id);
  return at < 0 ? '?' : `#${at}`;
}

/** One line of the trace. */
export function tracePrefill(step: string, facts: Record<string, unknown>): void {
  const parts = Object.entries(facts).map(([key, value]) => `${key}=${String(value)}`);
  // eslint-disable-next-line no-console
  console.info(`[prefill +${Math.round(performance.now())}ms] ${step}: ${parts.join(' ')}`);
}
