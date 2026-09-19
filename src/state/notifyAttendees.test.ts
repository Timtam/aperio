import { describe, expect, it } from 'vitest';
import { offersNotifyAttendees } from '@aperio/shared';

// The rule both editors share for "Notify attendees" (decisions 70a, 74a).
describe('offersNotifyAttendees', () => {
  const own = { attendees: ['bob@example.com'], organized_elsewhere: false };

  it('offers it for invitees on a calendar that schedules', () => {
    expect(
      offersNotifyAttendees({ supportsScheduling: true, attendees: ['bob@example.com'], original: null }),
    ).toBe(true);
    expect(
      offersNotifyAttendees({ supportsScheduling: false, attendees: ['bob@example.com'], original: null }),
    ).toBe(false);
  });

  it('keeps offering it when the edit removed every invitee', () => {
    // Bob may still get a cancellation (74a).
    expect(offersNotifyAttendees({ supportsScheduling: true, attendees: [], original: own })).toBe(true);
    // Nobody before, nobody now: nobody to tell.
    expect(
      offersNotifyAttendees({ supportsScheduling: true, attendees: [], original: { attendees: [] } }),
    ).toBe(false);
    expect(offersNotifyAttendees({ supportsScheduling: true, attendees: [], original: null })).toBe(false);
  });

  it('never offers it for a meeting someone else organizes', () => {
    expect(
      offersNotifyAttendees({
        supportsScheduling: true,
        attendees: ['bob@example.com'],
        original: { ...own, organized_elsewhere: true },
      }),
    ).toBe(false);
  });
});
