import { Platform } from 'react-native';

import { useScreenReaderEnabled } from '../a11y/useScreenReaderEnabled';

/**
 * True when the native VoiceOver pager owns the period heading (iOS + a screen
 * reader active). The `CalendarPager` then renders its OWN focusable period
 * header and announces period changes; the calendar screens use this to drop
 * their own visual heading + announcement so it isn't shown/announced twice.
 */
export function useCalendarPagerOwnsHeading(): boolean {
  const screenReader = useScreenReaderEnabled();
  return screenReader && Platform.OS === 'ios';
}
