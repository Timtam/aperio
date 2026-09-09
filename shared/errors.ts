/**
 * The message to put in front of a person when something failed.
 *
 * A rejected Tauri command or FFI bridge call arrives as an `Error`; a rejected
 * promise can carry anything at all. Both have to end up as a string, because
 * the string is what the screen reader says — and saying nothing is the one
 * outcome that must never happen.
 *
 * Four copies of this existed (three on mobile, one about to be written on the
 * desktop) before it was one.
 */
export function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}
