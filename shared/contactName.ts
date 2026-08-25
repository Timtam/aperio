// Deriving a contact's display name from its parts — the composition every
// other contacts app performs. Apple composes the shown name from the parts
// outright (there is no free display-name field), Google derives displayName
// server-side and ignores a written one, Outlook pre-fills and lets the user
// override. Aperio keeps its editable display name (adapters need a stored
// FN), so the editors follow the Outlook shape: SUGGEST the composed name and
// keep it in step with the parts for as long as the user hasn't typed their
// own — the editors detect that by comparing the current display name against
// the derivation of the current parts (equal ⇒ still automatic).

export interface ContactNameParts {
  namePrefix?: string | null;
  givenName?: string | null;
  familyName?: string | null;
  nameSuffix?: string | null;
  /** Fallback when there are no name parts at all — a company contact shows
   *  its organization, the same fallback Apple and the read-side adapters
   *  apply. */
  organization?: string | null;
}

/**
 * The composed display name for `parts`: prefix, given, family and suffix in
 * that order, space-joined ("Prof. Dr. Max Mustermann jun."). Deliberately no
 * comma before the suffix — the English "Smith, Jr." convention reads wrong in
 * German and the plain join reads fine in both. Empty when nothing is filled
 * in (the caller's required-field validation still applies).
 */
export function deriveDisplayName(parts: ContactNameParts): string {
  const name = [
    parts.namePrefix,
    parts.givenName,
    parts.familyName,
    parts.nameSuffix,
  ]
    .map((p) => (p ?? '').trim())
    .filter((p) => p.length > 0)
    .join(' ');
  if (name.length > 0) return name;
  return (parts.organization ?? '').trim();
}
