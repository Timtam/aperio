/**
 * Which detail rows a conference section shows, and in which order.
 *
 * A detected meeting can carry details from two places, and they are not the
 * same kind of thing:
 *
 * - **Derived** — a meeting number and a password Aperio recovered itself, out
 *   of a `tel:` DTMF payload or a SIP address. Aperio names these, so the label
 *   is translated.
 * - **Labelled** — `label: value` lines the invitation itself put next to the
 *   link, with the label exactly as the sender wrote it. That is how the
 *   details survive without a dictionary: a Webex invitation from an Exchange
 *   organiser carries no `tel:` and no `sip:` at all, and its meeting id sits
 *   behind whatever words the sender's site is configured with —
 *   "Besprechungs-ID", "Meeting number (access code)". The label is data.
 *
 * # Why this exists
 *
 * Both surfaces used to pick ONE list: `derived.length > 0 ? derived :
 * labelled`. So the moment Aperio recovered any meeting number at all, the
 * line the invitation actually contained was suppressed entirely.
 *
 * That is the wrong way round for the case that matters. A derived value comes
 * from parsing a tone sequence; a labelled value is what a person typed. When
 * they disagree, the parsed one is the more likely to be wrong — and it was the
 * only one shown, so there was nothing to notice the disagreement against. For
 * a screen-reader user the section IS the meeting details; a wrong number that
 * hides the right one is not a cosmetic problem.
 *
 * Both are shown now, derived first because it is the answer Aperio is
 * confident enough to have named.
 */

// One row of the conference section: a label and the value under it. Declared
// in Rust and generated from there (`cal_core::conferencing::ConferenceDetail`),
// because a detected meeting's own labelled lines arrive in exactly this shape
// and two hand-kept copies of one pair is the disease this file's whole
// neighbourhood is being treated for.
import type { ConferenceDetail } from './generated/ConferenceDetail';

/**
 * Derived rows first, then the invitation's own — minus any that repeats a
 * value already shown.
 *
 * Deduplicated on the VALUE, not the label: the two lists label the same thing
 * differently on purpose (Aperio's translated word against the sender's), so
 * comparing labels would never match and the common case — the invitation
 * stating the same number Aperio parsed — would read it out twice.
 *
 * Values are compared after trimming, because an invitation's line often
 * carries a trailing space the parsed form does not. Nothing else is
 * normalised: two numbers that differ by a space in the middle are two
 * different claims about the meeting, and showing both is the point.
 */
export function conferenceDetailRows(
  derived: readonly ConferenceDetail[],
  labelled: readonly ConferenceDetail[],
): ConferenceDetail[] {
  const shown = new Set(derived.map((d) => d.value.trim()));
  return [...derived, ...labelled.filter((l) => !shown.has(l.value.trim()))];
}
