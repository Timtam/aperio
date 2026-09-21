import { describe, expect, it } from 'vitest';
import { retiredPrivateReminders } from '@aperio/shared';

describe('retiredPrivateReminders', () => {
  it('empties the key and gives it no signature', () => {
    expect(retiredPrivateReminders('cal', 'S:item|ck1')).toEqual({
      calendar_id: 'cal',
      event_id: 'S:item|ck1',
      reminders: [],
      title: '',
      starts_at: '',
    });
  });
});
