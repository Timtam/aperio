import { describe, expect, it } from 'vitest';
import { privateReminderWrites } from '@aperio/shared';

const reminder = { kind: { type: 'relative', minutes_before: 60 }, sound: null };
const to = {
  calendar_id: 'cal',
  event_id: 'S:item|ck2',
  reminders: [reminder],
  title: 'Zahnarzt',
  starts_at: '2026-06-15T09:00:00.000Z',
};

describe('privateReminderWrites', () => {
  it('writes only the new row when the key did not change', () => {
    expect(privateReminderWrites({ to, from: { calendar_id: 'cal', event_id: 'S:item|ck2' } })).toEqual([
      to,
    ]);
  });

  it("leaves the old key alone when that key's event lives on", () => {
    expect(privateReminderWrites({ to, from: null })).toEqual([to]);
  });

  it('retires the old key without a signature when the provider minted a new id', () => {
    expect(privateReminderWrites({ to, from: { calendar_id: 'cal', event_id: 'S:item|ck1' } })).toEqual([
      to,
      { calendar_id: 'cal', event_id: 'S:item|ck1', reminders: [], title: '', starts_at: '' },
    ]);
  });

  it('retires the old key without a signature after a move to another calendar', () => {
    expect(privateReminderWrites({ to, from: { calendar_id: 'home', event_id: 'ev' } })).toEqual([
      to,
      { calendar_id: 'home', event_id: 'ev', reminders: [], title: '', starts_at: '' },
    ]);
  });
});
