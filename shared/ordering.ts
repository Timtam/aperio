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
 * core: of the twenty-one `localeCompare` calls in this app, ten were on
 * machine strings and never needed collation at all (DESIGN §4.3, TODO A11).
 *
 * For TEXT a person reads — a task title, a list name, a contact — keep using
 * a real collation. Those are a different question and a decided one.
 */
export function compareMachineStrings(a: string, b: string): number {
  // Not `a.localeCompare(b)` and not `a < b ? -1 : …` written inline at ten
  // call sites: one named function is what makes the distinction reviewable.
  if (a === b) return 0;
  return a < b ? -1 : 1;
}

/**
 * How TEXT a person reads is ordered — installed by the surface, not imported.
 *
 * The rule itself lives in `cal_core::collation`; what differs per surface is
 * only how Rust is reached. The desktop calls it through WebAssembly compiled
 * into its webview, mobile through a synchronous Expo `Function`, and a future
 * native frontend will link it directly. Each installs its own door here at
 * startup, and everything in this package then compares the same way.
 *
 * Installed rather than passed as a parameter for a practical reason:
 * `sectionOrder` (and, until they moved into the core, `taskOrder` and
 * `buildEntries`) are called from nine layers of view code and
 * from `buildEntries`, which already carries nine parameters. Threading a
 * language through all of that would put the choice in front of every future
 * caller, which is how it drifted in the first place.
 *
 * The LANGUAGE is bound at install time, and it is the language the user chose
 * in Aperio. Every `localeCompare` this replaces passed `undefined` and so
 * followed the operating system — German text ordered by English rules on an
 * English machine. A surface re-installs when the user changes language.
 */
export interface TextCollation {
  /** Names of things: accounts, containers, contacts, day markers. */
  compareNames(a: string, b: string): number;
  /** Text the user wrote: task titles, section names. */
  compareTitles(a: string, b: string): number;
}

let installed: TextCollation | null = null;

/** Bind this surface's door into the core. Call again to change language. */
export function installTextCollation(collation: TextCollation): void {
  installed = collation;
}

function active(): TextCollation {
  if (installed === null) {
    // Loud, not a codepoint fallback. A fallback here would be a second
    // ordering rule that only shows up as "the list looks odd on one device",
    // which is exactly what installing one shared rule is meant to end.
    throw new Error(
      'text collation used before installTextCollation() — the surface must ' +
        'install its door into cal-core during startup',
    );
  }
  return installed;
}

/** Compare two NAMES. Case- and accent-insensitive. */
export function compareNames(a: string, b: string): number {
  return active().compareNames(a, b);
}

/** Compare two TITLES. Digit runs order by value; case separates. */
export function compareTitles(a: string, b: string): number {
  return active().compareTitles(a, b);
}
