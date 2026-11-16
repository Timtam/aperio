/** Result of `CalFfi.appVersion`: the installed app's own version and build
 *  number, read from the bundle (iOS) or the package (Android). Empty strings
 *  when the platform does not say. */
export type AppVersion = {
  version: string;
  build: string;
};

/** Result of `CalFfi.parseAttendee`, produced by the Rust cal-core parser. */
export type ParsedAttendee = {
  /** Display name, or `null` for a bare email entry. */
  name: string | null;
  /** The email address. */
  email: string;
};
