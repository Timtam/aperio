// When two event titles mean the same appointment.
//
// This is the TypeScript half of one rule. The other half is
// `cal_core::normalized_title`, and both answer the table in
// `crates/cal-core/tests/fixtures/normalizedTitle.json` — which is what keeps
// them from drifting apart again.
//
// # Why there are two halves at all
//
// The rule was written FIVE times: three times here (`groupSuggestions.ts`,
// `suggestGroupMate.ts` and `healEventGroups.ts`, byte for byte identical) and
// twice in `host-core` (`event_anchor::plan_repairs` and the reminder
// scheduler's own anchor repair). The two languages spelled it differently:
// TypeScript collapsed inner runs of whitespace, Rust only trimmed the ends.
//
// Each copy was self-consistent — every one of them puts BOTH titles of a
// comparison through itself — so no single comparison ever got two answers, and
// nothing looked broken. The damage was a CHAIN across the two languages: this
// side offers to group two events because it reads their titles as the same,
// and if the title's inner spacing later changes, Rust's repair no longer
// re-finds the member and it drops out of the group in silence. Grouped by one
// reading of "same", lost by another.
//
// This half is not a door into the core, unlike `taskStatus.ts` or
// `collation.ts`, and that is deliberate: its callers are themselves on their
// way into `cal-core`, and they take this file with them when they go. A door
// built for the one step in between would be thrown away on the next.
//
// One of the three has already gone: `healEventGroups.ts` was deleted when the
// host took over anchoring group membership, and its rule is now
// `cal_core::event_anchor`.

/**
 * Whether a character is a gap between words.
 *
 * Written out rather than left to `\s`, because the two languages' built-in
 * notions of whitespace are NOT the same set and either one alone would put
 * the halves back into disagreement:
 *
 * - JavaScript's `\s` counts U+FEFF, the byte-order mark. Rust's
 *   `char::is_whitespace` does not, and neither does `String::trim`.
 * - Rust counts U+0085 (NEL, a line break older systems still emit).
 *   JavaScript's `\s` does not.
 *
 * The set here is Unicode `White_Space`, which is what Rust means. It is spelled
 * as an adjustment to `\s` rather than as `\p{White_Space}` because that escape
 * needs a regex engine feature this code cannot assume on every mobile runtime.
 */
function isGap(ch: string): boolean {
  return ch !== '\uFEFF' && (/\s/.test(ch) || ch === '\u0085');
}

/**
 * Two titles that a person would call the same one.
 *
 * Case, padding and the WIDTH of the gaps between words are all noise: a title
 * is typed by people and re-flowed by providers, and "Wochen  planung" with two
 * spaces is not a different appointment from "Wochen planung" with one.
 *
 * The generous reading wins, and deliberately: it errs towards keeping a member
 * in a group rather than towards losing one without a word.
 *
 * Two details are not free-hand:
 *
 * - iteration is over CODE POINTS (`for…of`), not UTF-16 units, so a character
 *   outside the basic plane is not taken apart;
 * - lowercasing happens to the whole collapsed string at once, never character
 *   by character, because a Greek sigma at the end of a word lowercases to ς
 *   and one inside a word to σ — a distinction that depends on what follows and
 *   that a per-character fold gets wrong. Rust's `str::to_lowercase` draws the
 *   same distinction, so whole-string on both sides is the spelling that agrees.
 */
export function normalizedTitle(title: string): string {
  let collapsed = '';
  let inGap = false;
  for (const ch of title) {
    if (isGap(ch)) {
      inGap = true;
      continue;
    }
    // A gap before the first character is padding, not a gap between words —
    // which is how the leading and trailing trim happens without a `trim()`
    // call that would disagree with Rust about the byte-order mark.
    if (inGap && collapsed.length > 0) collapsed += ' ';
    inGap = false;
    collapsed += ch;
  }
  return collapsed.toLowerCase();
}
