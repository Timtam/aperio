/**
 * Comparing strings that are not text.
 *
 * An ISO day key, an RFC-3339 instant, an id: these are MACHINE strings. No
 * human reads them, they have a fixed shape, and the order that matters is the
 * one their format already encodes — `2026-01-02` before `2026-01-10` because
 * of what the digits mean, not because of any language.
 *
 * They were being compared with `localeCompare`, which is a language-aware
 * collation. That is wrong in a quiet way. It happens to give the right answer
 * for fixed-shape digit strings, so nothing has ever looked broken — but it
 * makes the ordering of a task list depend on the reader's locale, and it pays
 * for a collation lookup per comparison in code that runs inside `Array.sort`
 * during render.
 *
 * The distinction also decides how much of the ordering rule can move into the
 * core: of the twenty-one `localeCompare` calls in this app, eleven were on
 * machine strings and never needed collation at all (DESIGN §4.3, TODO A11).
 *
 * For TEXT a person reads — a task title, a list name, a contact — keep using
 * a real collation. Those are a different question and a decided one.
 */
export function compareMachineStrings(a: string, b: string): number {
  // Not `a.localeCompare(b)` and not `a < b ? -1 : …` written inline at twenty
  // call sites: one named function is what makes the distinction reviewable.
  if (a === b) return 0;
  return a < b ? -1 : 1;
}
