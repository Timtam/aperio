/**
 * The user-pref key a calendar's default reminders live under.
 *
 * One builder for both apps, because this string is a CONTRACT with the Rust
 * host: `host_core::reminders::configured_calendar_default_reminders` builds
 * the same key with `format!`, and a mismatch is completely silent — the
 * settings panel still shows what it saved, because it reads back its own
 * writer, while the host finds nothing and every default reminder of that
 * calendar stops existing.
 *
 * It used to be written out three times: a prefix+suffix pair on the desktop,
 * a template literal on mobile, and the `format!` in Rust. The two languages
 * cannot share a constant, so they share a FIXTURE instead —
 * `shared/contracts/calendarDefaultReminders.json` carries the template, and
 * both suites check themselves against it.
 *
 * The `calendar.` prefix is also what puts the key on the sync whitelist, so
 * it must survive any renaming: a key outside the prefix would save locally
 * and never reach the user's other devices.
 */
export function calendarDefaultRemindersKey(calendarId: string): string {
  return `calendar.${calendarId}.defaultReminders`;
}

/**
 * Whether the editor is telling the host that NO reminder choice was made, so
 * the calendar's attached defaults may still be written into the appointment.
 *
 * The wire flag is `use_calendar_defaults`, and it is optional on all four
 * sides — `#[serde(default)]` in both hosts, an optional field in both request
 * types. That makes forgetting it indistinguishable from declining it: the
 * appointment saves, the editor looks right, and the alarm the user configured
 * simply never reaches the provider. Nothing but a phone staying quiet would
 * say otherwise, which is why both editors ask the same function rather than
 * each spelling the rule out.
 *
 * Untouched AND empty is the only case. A list the user emptied is a decision
 * — "no reminder" — and an appointment that carries reminders has its own.
 */
export function madeNoReminderChoice(
  remindersTouched: boolean,
  remindersOnTheWire: readonly unknown[],
): boolean {
  return !remindersTouched && remindersOnTheWire.length === 0;
}
