// A list as the language writes one: "a, b and c" / „a, b und c“.
//
// The words were first needed by the recurrence summary ("on Monday, Tuesday
// and Friday") and live under its keys; any sentence that names several
// things joins them here, so there is one way to write a list.

type Translate = (key: string, values?: Record<string, unknown>) => string;

export function joinNames(names: readonly string[], t: Translate): string {
  if (names.length === 0) return '';
  if (names.length === 1) return names[0];
  const separator = t('recurrenceSummary.list.separator');
  const rest = names.slice(0, -1).join(separator);
  return t('recurrenceSummary.list.and', { rest, last: names[names.length - 1] });
}
