/** Result of `CalFfi.parseAttendee`, produced by the Rust cal-core parser. */
export type ParsedAttendee = {
  /** Display name, or `null` for a bare email entry. */
  name: string | null;
  /** The email address. */
  email: string;
};
