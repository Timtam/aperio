// Live Activities for appointments (DESIGN-live-activities.md): the reminder
// pass's door. On the iPhone ActivityKit, on Android the native ongoing
// notification; both from the same scan as the notification list, and both
// saying which reminders they stand for — those, and only those, Expo leaves
// out.

import { Platform } from 'react-native';

import type { UpcomingReminder } from '../api/reminders';
import { androidRemindersAndActivities } from './androidActivities';
import { iosRemindersAndActivities } from './iosActivities';

export { reminderKeyId } from './reminderKey';
export { startFromLastPlan } from './iosActivities';

export function remindersAndActivities(
  horizonMinutes: number,
  hidden: ReadonlySet<string>,
  channels: { channelFor: (r: UpcomingReminder) => Promise<string>; defaultChannel: string },
): Promise<{ items: UpcomingReminder[]; covered: Set<string> }> {
  return Platform.OS === 'android'
    ? androidRemindersAndActivities(horizonMinutes, hidden, channels)
    : iosRemindersAndActivities(horizonMinutes, hidden);
}
