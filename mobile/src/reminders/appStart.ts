// "On next app start" reminders on the phone: what is due at this opening of
// the app rings at once and is remembered (DESIGN §14.1). The rule — which
// entries, which moment, once per device — is the core's
// (`host_core::reminders::enumerate_app_start_triggers`); this only carries it
// out, with the platform's parts handed in so it can be tested without them
// (`src/state/appStartReminders.phone.test.ts`).

import type { UpcomingReminder } from '../api/reminders';

export interface AppStartDeps {
  /** Notifications may be shown. */
  permitted: () => Promise<boolean>;
  /** What is due at this opening (`appStartRemindersJson`). */
  due: () => Promise<UpcomingReminder[]>;
  /** Show one now, as a scheduled reminder would have rung. */
  show: (reminder: UpcomingReminder) => Promise<void>;
  /** Remember what was shown (`markAppStartRung`). */
  mark: (shown: UpcomingReminder[]) => Promise<void>;
}

/**
 * Ring what is due and remember what was shown. Returns how many rang.
 *
 * Only what was shown is remembered: without permission nothing is, so the
 * first opening after it is granted rings them, and one the system refused to
 * show is tried again at the next opening.
 */
export async function ringAppStartReminders(deps: AppStartDeps): Promise<number> {
  if (!(await deps.permitted())) return 0;
  const shown: UpcomingReminder[] = [];
  for (const reminder of await deps.due()) {
    try {
      await deps.show(reminder);
      shown.push(reminder);
    } catch {
      // Not remembered: the next opening tries it again.
    }
  }
  if (shown.length > 0) await deps.mark(shown);
  return shown.length;
}
