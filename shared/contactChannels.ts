/** Labelled contact channels — emails, phone numbers, websites.
 *
 *  Every provider Aperio talks to files these under some kind of label, and
 *  each one calls it something different: vCard puts it in a `TYPE` parameter
 *  (or an Apple `X-ABLabel` group), Exchange makes it the entry `Key`, Graph
 *  makes it the name of the collection the value sits in, Google types each
 *  entry directly. Aperio keeps one shape for all of them — a value with an
 *  optional free-text label — and lets each adapter translate at its own edge.
 *
 *  The wire is deliberately forgiving: a plain string is still a legal
 *  channel, because that is what every contact stored before labels existed
 *  looks like, and the Rust side deserialises both shapes.
 *
 *  Typed structurally so the desktop and mobile `Contact` shapes (which live
 *  in their own api layers) both satisfy it — same arrangement as
 *  `formatAttendee`. */

/** A channel as it arrives from the backend: either the modern object or a
 *  bare string from a contact written before labels existed. */
export type WireContactValue = string | { value: string; label?: string | null };

/** A channel in the shape the editors work with. */
export interface ContactValue {
  value: string;
  label: string | null;
}

/** Labels Aperio offers in its pickers. They are suggestions, not a closed
 *  set — CardDAV and Google both carry whatever word the user typed, so the
 *  editors keep a free-text option beside these and a custom label survives a
 *  round-trip on those providers.
 *
 *  Kept lower-case ASCII on the wire and translated for display, so the same
 *  contact reads "privat" in German and "home" in English without the stored
 *  value changing under the user. */
export const CONTACT_LABELS = ['home', 'work', 'mobile', 'fax', 'other'] as const;

export type KnownContactLabel = (typeof CONTACT_LABELS)[number];

/** Aliases the providers use for the labels above, so a contact that arrives
 *  from Exchange as `cell` or from a German vCard as `privat` lands on the
 *  same picker entry rather than looking like a custom label. */
const LABEL_ALIASES: Record<string, KnownContactLabel> = {
  cell: 'mobile',
  business: 'work',
  privat: 'home',
  dienstlich: 'work',
  homefax: 'fax',
  workfax: 'fax',
  businessfax: 'fax',
  otherfax: 'fax',
};

/** Normalise one wire channel into the editor shape. Blank labels become
 *  `null` — "no label" and "a label that is the empty string" are the same
 *  thing to a reader, and only one of them round-trips cleanly. */
export function toContactValue(raw: WireContactValue): ContactValue {
  if (typeof raw === 'string') {
    return { value: raw, label: null };
  }
  const label = raw.label?.trim();
  return { value: raw.value ?? '', label: label ? label : null };
}

/** Normalise a whole channel list, dropping entries with no value — an empty
 *  row is a UI artefact, not something to store. */
export function toContactValues(raw: readonly WireContactValue[] | null | undefined): ContactValue[] {
  return (raw ?? []).map(toContactValue).filter((v) => v.value.trim().length > 0);
}

/** Back to the wire. Labels stay as typed so a custom one survives; the value
 *  is trimmed because trailing whitespace in a phone number is never meant. */
export function fromContactValues(values: readonly ContactValue[]): { value: string; label?: string }[] {
  return values
    .map((v) => ({ value: v.value.trim(), label: v.label?.trim() ?? '' }))
    .filter((v) => v.value.length > 0)
    .map((v) => (v.label ? { value: v.value, label: v.label } : { value: v.value }));
}

/** The first usable value in a channel list, as a plain string — what a list
 *  row or an attendee chip shows when there is only room for one. `null` when
 *  the contact has none. */
export function primaryChannelValue(
  raw: readonly WireContactValue[] | null | undefined,
): string | null {
  for (const entry of raw ?? []) {
    const value = (typeof entry === 'string' ? entry : entry.value)?.trim();
    if (value) return value;
  }
  return null;
}

/** The picker entry a stored label belongs to, or `null` when it is the
 *  user's own word and belongs in the free-text field. */
export function knownLabel(label: string | null | undefined): KnownContactLabel | null {
  if (!label) return null;
  const key = label.trim().toLowerCase();
  if ((CONTACT_LABELS as readonly string[]).includes(key)) {
    return key as KnownContactLabel;
  }
  return LABEL_ALIASES[key] ?? null;
}
