// User-preferences api-client — the generic synced key/value store, the mobile
// twin of the desktop user_pref commands. Values are opaque strings; structured
// prefs are JSON-serialised by the caller (the JSON helpers below). A write to a
// §19.2.1-whitelisted key (locale, view.weekStart, appearance.*, sound.*,
// reminders.defaults.*, calendar.<id>.defaultReminders, …) is appended to the
// sync log Rust-side, so it propagates to the user's other devices; local-only
// keys (sidebar state, etc.) stay on-device. The settings panels build on this.

import CalFfi from '../../modules/cal-ffi';

/** Read a preference's raw string value, or null when unset. */
export const getUserPref = (key: string): Promise<string | null> =>
  CalFfi.getUserPref(key);

/** Upsert a preference's raw string value. A whitelisted key also syncs. */
export const setUserPref = (key: string, value: string): Promise<void> =>
  CalFfi.setUserPref(key, value);

/** Delete a preference (a whitelisted key also syncs the deletion). */
export const deleteUserPref = (key: string): Promise<void> =>
  CalFfi.deleteUserPref(key);

/** Read + JSON-parse a structured preference. Returns null when unset or when
 *  the stored value isn't valid JSON (treated as absent rather than throwing). */
export const getUserPrefJson = async <T>(key: string): Promise<T | null> => {
  const raw = await CalFfi.getUserPref(key);
  if (raw == null) return null;
  try {
    return JSON.parse(raw) as T;
  } catch {
    return null;
  }
};

/** JSON-serialise + store a structured preference. */
export const setUserPrefJson = (key: string, value: unknown): Promise<void> =>
  CalFfi.setUserPref(key, JSON.stringify(value));
